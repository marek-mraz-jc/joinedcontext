#![forbid(unsafe_code)]
//! The native API as one OData 4.01 service (owner order 2026-11-07): `$metadata` lists every
//! registry operation (entity sets, bound and unbound actions and functions), actions answer the
//! same through OData, MCP and REST, errors are OData error JSON carrying the RFC 9457 slug.

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
const BASE: &str = "/api/v1/realms/dev/odata";
const STEWARD: &str = "de300000-0000-4000-8000-000000000001";
const ANALYST: &str = "de300000-0000-4000-8000-000000000010";

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
        let now = jc_time::TimestampTz::now().as_second();
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

fn b64url(bytes: &[u8]) -> String {
    use base64::engine::{Engine, general_purpose::URL_SAFE_NO_PAD};
    URL_SAFE_NO_PAD.encode(bytes)
}

/// A WebAuthn registration response ("none" attestation, a fresh ES256 key) for `challenge`, the
/// relying party `rp_id` and the origin of the test mount.
fn passkey_registration(challenge: &str, rp_id: &str, cred_id: &[u8]) -> Value {
    use aws_lc_rs::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};
    use sha2::{Digest, Sha256};
    let rng = aws_lc_rs::rand::SystemRandom::new();
    let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref()).unwrap();
    let public = pair.public_key().as_ref();
    // COSE_Key {1: 2 (EC2), 3: -7 (ES256), -1: 1 (P-256), -2: x, -3: y}
    let mut cose = vec![0xa5, 0x01, 0x02, 0x03, 0x26, 0x20, 0x01, 0x21, 0x58, 0x20];
    cose.extend_from_slice(&public[1..33]);
    cose.extend_from_slice(&[0x22, 0x58, 0x20]);
    cose.extend_from_slice(&public[33..65]);
    let mut auth_data = Sha256::digest(rp_id.as_bytes()).to_vec();
    auth_data.push(AUTH_DATA_FLAG_UP | AUTH_DATA_FLAG_UV | AUTH_DATA_FLAG_AT);
    auth_data.extend_from_slice(&1u32.to_be_bytes());
    auth_data.extend_from_slice(&[0u8; 16]);
    auth_data.extend_from_slice(&u16::try_from(cred_id.len()).unwrap().to_be_bytes());
    auth_data.extend_from_slice(cred_id);
    auth_data.extend_from_slice(&cose);
    // {"fmt": "none", "attStmt": {}, "authData": bytes}
    let mut att = b"\xa3\x63fmt\x64none\x67attStmt\xa0\x68authData\x58".to_vec();
    att.push(u8::try_from(auth_data.len()).unwrap());
    att.extend_from_slice(&auth_data);
    let client_data = json!({ "type": "webauthn.create", "challenge": challenge,
                              "origin": "https://idm.dev.example.test" });
    json!({
        "id": b64url(cred_id), "rawId": b64url(cred_id), "type": "public-key",
        "response": {
            "clientDataJSON": b64url(client_data.to_string().as_bytes()),
            "attestationObject": b64url(&att),
            "transports": ["internal"]
        }
    })
}

/// Step 1 of RegisterPasskey for the analyst: the challenge, after checking the offered rp id.
async fn passkey_challenge(api: &Api, token: &str, rp_id: &str) -> String {
    let (_, _, opts, _) = api
        .call(
            Method::POST,
            &format!("{BASE}/People('{ANALYST}')/RegisterPasskey"),
            token,
            Some(json!({})),
        )
        .await;
    assert_eq!(opts["value"]["rp"]["id"], rp_id, "{opts}");
    opts["value"]["challenge"].as_str().unwrap().to_owned()
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
                        format!("Collection(JoinedContext.Kaisatsu.{b})")
                    } else {
                        format!("JoinedContext.Kaisatsu.{b}")
                    };
                    assert!(element.contains(&format!("Name=\"bindingParameter\" Type=\"{ty}\"")));
                } else {
                    let import = if tag == "Action" {
                        "ActionImport"
                    } else {
                        "FunctionImport"
                    };
                    assert!(xml.contains(&format!(
                        "<{import} Name=\"{}\" {tag}=\"JoinedContext.Kaisatsu.{}\"/>",
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
    let uri = format!("{BASE}/People('{ANALYST}')/JoinedContext.Kaisatsu.ResetCredentials");
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
        kaisatsu_crypto::verify_and_maybe_rehash("OtherAnalystPass2!y", pw)
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
    // No password at all is refused whatever the realm's policy.
    let (st, _, _, raw) = api
        .call(
            Method::POST,
            &uri,
            &admin,
            Some(json!({ "temporary": true })),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{raw}");

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
            &format!("{BASE}/People('missing-id')/JoinedContext.Kaisatsu.ResetCredentials"),
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
            &format!("{BASE}/Realms('dev')/JoinedContext.Kaisatsu.Export(withUsers=false)"),
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
            &format!("{BASE}/Realms('other')/JoinedContext.Kaisatsu.Export()"),
            &admin,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    // A function is not POSTed and an action is not GET.
    let (st, _, _, _) = api
        .call(
            Method::POST,
            &format!("{BASE}/Realms('dev')/JoinedContext.Kaisatsu.Export"),
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
            &format!("{BASE}/SigningKeys/JoinedContext.Kaisatsu.Rotate"),
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
async fn realm_apply_writes_only_the_realm_it_is_bound_to() {
    let api = Api::new().await;
    let admin = api.admin().await;
    let dev_id = api.store.get_realm("dev").await.unwrap().unwrap().id;
    let (st, _, _, raw) = api
        .call(
            Method::POST,
            &format!("{BASE}/Realms('dev')/Apply"),
            &admin,
            Some(json!({ "configuration": { "id": "master", "realm": "master", "displayName": "Bound" } })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{raw}");
    assert!(api.store.get_realm("master").await.unwrap().is_none());
    let dev = api.store.get_realm("dev").await.unwrap().unwrap();
    assert_eq!(dev.id, dev_id);
    assert_eq!(dev.display_name, "Bound");
}

#[tokio::test]
async fn credentials_are_enrolled_only_through_the_account_console() {
    let api = Api::new().await;
    let app = api.token(ANALYST, json!({ "azp": "some-org-app" })).await;
    for op in ["EnrolTotp", "GenerateRecoveryCodes", "RegisterPasskey"] {
        let (st, _, _, raw) = api
            .call(
                Method::POST,
                &format!("{BASE}/People('{ANALYST}')/{op}"),
                &app,
                Some(json!({})),
            )
            .await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{op}: {raw}");
    }
    let creds = api.store.list_credentials("dev", ANALYST).await.unwrap();
    assert!(
        !creds
            .iter()
            .any(|c| c.credential_type == "recovery-authn-codes")
    );
    let console = api
        .token(ANALYST, json!({ "azp": "account-console" }))
        .await;
    let (st, _, _, raw) = api
        .call(
            Method::POST,
            &format!("{BASE}/People('{ANALYST}')/GenerateRecoveryCodes"),
            &console,
            Some(json!({})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{raw}");
}

/// Credential and registry operations refuse what the caller may not do and name what is wrong.
#[tokio::test]
async fn credential_and_registry_operations_refuse_with_a_reason() {
    let api = Api::new().await;
    let admin = api.admin().await;
    let me = api
        .token(ANALYST, json!({ "azp": "account-console" }))
        .await;
    let post = |uri: String, token: String, body: Value| {
        let api = &api;
        async move { api.call(Method::POST, &uri, &token, Some(body)).await }
    };
    // Another person's recovery codes and passkeys answer 404 like a missing person.
    for op in ["GenerateRecoveryCodes", "RegisterPasskey"] {
        let (st, _, _, raw) = post(
            format!("{BASE}/People('{STEWARD}')/{op}"),
            me.clone(),
            json!({}),
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{op}: {raw}");
    }
    // A realm administrator may start a passkey registration for a person.
    let (st, _, opts, raw) = post(
        format!("{BASE}/People('{ANALYST}')/RegisterPasskey"),
        admin.clone(),
        json!({}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{raw}");
    assert!(opts["value"]["challenge"].is_string(), "{opts}");
    // TOTP step 2 with a secret shorter than step 1's key is refused before any code check.
    let (st, _, err, _) = post(
        format!("{BASE}/People('{ANALYST}')/EnrolTotp"),
        me.clone(),
        json!({ "secret": "SHORT", "code": "123456" }),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(err.to_string().contains("160-bit"), "{err}");
    // An unknown passkey and an unknown consent answer 404.
    let (st, _, err, _) = post(
        format!("{BASE}/WebAuthnCredentials('no-such-passkey')/DeletePasskey"),
        admin.clone(),
        json!({}),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{err}");
    let (st, _, err, _) = post(
        format!("{BASE}/Consents('no-such-consent')/Revoke"),
        admin.clone(),
        json!({}),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{err}");
    // Allowed names an unknown operation and a set it does not judge.
    for (q, needle) in [
        ("operation='frobnicate'", "unknown operation: frobnicate"),
        (
            "operation='read',object='Nonsense'",
            "does not judge objects of Nonsense",
        ),
    ] {
        let (st, _, err, raw) = api
            .call(Method::GET, &format!("{BASE}/Allowed({q})"), &admin, None)
            .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{q}: {raw}");
        assert!(err.to_string().contains(needle), "{q}: {err}");
    }
    // Allowed also takes the action verbs.
    for verb in ["create", "self"] {
        let (st, _, ans, raw) = api
            .call(
                Method::GET,
                &format!("{BASE}/Allowed(operation='{verb}',object='People(''{ANALYST}'')')"),
                &admin,
                None,
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{verb}: {raw}");
        assert!(ans["value"]["allowed"].is_boolean(), "{verb}: {ans}");
    }
    // Recovery codes and TOTP are the person's own: an administrator gets 404.
    for op in ["GenerateRecoveryCodes", "EnrolTotp"] {
        let (st, _, _, raw) = post(
            format!("{BASE}/People('{ANALYST}')/{op}"),
            admin.clone(),
            json!({}),
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{op}: {raw}");
    }
    // New recovery codes replace the old set.
    for _ in 0..2 {
        let (st, _, _, raw) = post(
            format!("{BASE}/People('{ANALYST}')/GenerateRecoveryCodes"),
            me.clone(),
            json!({}),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{raw}");
    }
    let sets = api.store.list_credentials("dev", ANALYST).await.unwrap();
    assert_eq!(
        sets.iter()
            .filter(|c| c.credential_type == "recovery-authn-codes")
            .count(),
        1
    );
    // An administrator maps a client's roles to a person.
    let client = api
        .store
        .get_client_by_client_id("dev", "portal-api")
        .await
        .unwrap()
        .unwrap();
    let (st, _, _, raw) = post(
        format!("{BASE}/People('{ANALYST}')/MapClientRoles"),
        admin.clone(),
        json!({ "clientUuid": client.id, "roles": ["reader"] }),
    )
    .await;
    assert!(st.is_success(), "{st} {raw}");
    let mapped = api
        .store
        .list_role_mappings_for_principal("dev", "user", ANALYST, Some(&client.id))
        .await
        .unwrap();
    assert!(mapped.iter().any(|m| m.role_name == "reader"), "{mapped:?}");
    // Apply with prune as a dry run reports and writes nothing.
    let (st, _, body, raw) = post(
        format!("{BASE}/Realms('dev')/Apply"),
        admin.clone(),
        json!({ "configuration": { "displayName": "Pruned" }, "dryRun": true, "prune": true }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{raw}");
    assert_eq!(body["value"]["dryRun"], true);
    assert_ne!(
        api.store
            .get_realm("dev")
            .await
            .unwrap()
            .unwrap()
            .display_name,
        "Pruned"
    );
    // A person without realm administration cannot export the realm: 404 like another realm.
    let (st, _, _, raw) = api
        .call(
            Method::GET,
            &format!("{BASE}/Realms('dev')/JoinedContext.Kaisatsu.Export()"),
            &me,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{raw}");
}

/// A client named like the realm's own, but owned by an Organization, carries none of its trust.
#[tokio::test]
async fn an_organizations_client_named_account_console_enrols_nothing() {
    let api = Api::new().await;
    let mut console = api
        .store
        .get_client_by_client_id("dev", "account-console")
        .await
        .unwrap()
        .unwrap();
    console.org_id = Some("helsinki".into());
    api.store.upsert_client(console).await.unwrap();
    let token = api
        .token(ANALYST, json!({ "azp": "account-console" }))
        .await;
    let (st, _, _, raw) = api
        .call(
            Method::POST,
            &format!("{BASE}/People('{ANALYST}')/GenerateRecoveryCodes"),
            &token,
            Some(json!({})),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{raw}");
}

/// Every write operation of the registry leaves an admin event (S3-35, AUD-02): one row per
/// operation of this table, and no write operation of the registry outside it.
#[tokio::test]
async fn every_write_operation_leaves_an_admin_event() {
    // Excluded, with the reason: realm_apply writes one event per changed object through
    // kaisatsu-import (realm_apply_writes_only_the_realm_it_is_bound_to); passkey_register's
    // second step needs a signed authenticator attestation, and its event is written by the
    // same code path after insert_webauthn_credential; the domain operations need a DNS answer.
    const NOT_IN_TABLE: &[&str] = &[
        "realm_apply",
        "passkey_register",
        "domain_request_verification",
        "domain_verify",
    ];
    let api = Api::new().await;
    let admin = api.admin().await;
    let me = api
        .token(ANALYST, json!({ "azp": "account-console" }))
        .await;
    let store = &api.store;
    let group = store.list_groups("dev", None, None, false).await.unwrap()[0]
        .id
        .clone();
    let client = store
        .get_client_by_client_id("dev", "portal-api")
        .await
        .unwrap()
        .unwrap();
    let analyst = store
        .get_user_by_id("dev", None, ANALYST)
        .await
        .unwrap()
        .unwrap();
    store
        .upsert_consent(ConsentRecord {
            id: "consent-audit".into(),
            realm_id: "dev".into(),
            org_id: analyst.org_id.clone(),
            user_id: ANALYST.into(),
            client_id: "portal-api".into(),
            granted_client_scopes: vec!["profile".into()],
            created_date: 0,
            last_updated_date: 0,
        })
        .await
        .unwrap();
    let outbox: EmailOutboxRecord = serde_json::from_value(json!({
        "id": "outbox-audit", "realmId": "dev", "orgId": analyst.org_id, "userId": ANALYST,
        "toAddress": "analyst@example.test", "locale": "en", "templateKind": "execute-actions",
        "actionJti": "jti-audit", "lifespanSecs": 600, "issuer": ISSUER, "status": "failed",
        "attempts": 5, "nextAttemptAt": "2026-01-01T00:00:00Z", "createdAt": "2026-01-01T00:00:00Z"
    }))
    .unwrap();
    store.enqueue_outbox(outbox).await.unwrap();
    store
        .insert_webauthn_credential(WebAuthnCredentialRecord {
            id: "pk-audit".into(),
            realm_id: "dev".into(),
            org_id: analyst.org_id.clone(),
            user_id: ANALYST.into(),
            user_handle: "handle-audit".into(),
            credential_id: "cred-audit".into(),
            public_key_cose_json: "{}".into(),
            cose_alg: -7,
            sign_count: 0,
            uv_initialized: false,
            backup_eligible: false,
            backup_state: false,
            transports: Vec::new(),
            attestation_type: "none".into(),
            rp_id: "idm.dev.example.test".into(),
            user_label: None,
            created_at: jc_time::TimestampTz::now(),
            last_used_at: None,
        })
        .await
        .unwrap();
    let created_id = "de300000-0000-4000-8000-0000000000aa";
    let mut realm = store.get_realm("dev").await.unwrap().unwrap();
    realm.smtp_server = Some(SmtpServerConfig::default());
    store.upsert_realm(realm.clone()).await.unwrap();
    let totp_secret = "JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP";
    let totp = CredentialRecord {
        credential_type: "otp".into(),
        secret_data: json!({ "value": totp_secret }).to_string(),
        credential_data: json!({ "digits": realm.otp_policy_digits, "period": realm.otp_policy_period,
                                 "algorithm": realm.otp_policy_algorithm, "secretEncoding": "BASE32" })
        .to_string(),
        ..Default::default()
    };
    let code =
        generate_totp_code_for_credential(&totp, jc_time::TimestampTz::now().as_second() as u64)
            .unwrap();
    let person = |id: &str, op: &str| format!("{BASE}/People('{id}')/{op}");
    // (operation, method, uri, caller, body, resource type of its event)
    type Row<'a> = (&'a str, Method, String, &'a str, Option<Value>, &'a str);
    let table: Vec<Row> = vec![
        (
            "person_create",
            Method::POST,
            format!("{BASE}/People"),
            &admin,
            Some(json!({ "id": created_id, "username": "audit-person" })),
            "USER",
        ),
        (
            "person_update",
            Method::PATCH,
            format!("{BASE}/People('{created_id}')"),
            &admin,
            Some(json!({ "firstName": "Audit" })),
            "USER",
        ),
        (
            "member_add",
            Method::POST,
            person(created_id, "AddToGroup"),
            &admin,
            Some(json!({ "groupId": group })),
            "GROUP_MEMBERSHIP",
        ),
        (
            "role_map",
            Method::POST,
            person(created_id, "MapClientRoles"),
            &admin,
            Some(json!({ "clientUuid": client.id, "roles": [] })),
            "CLIENT_ROLE_MAPPING",
        ),
        (
            "credential_reset",
            Method::POST,
            person(created_id, "ResetCredentials"),
            &admin,
            Some(json!({ "value": "Audit-Pass-2026-long", "temporary": true })),
            "USER",
        ),
        (
            "send_required_actions",
            Method::POST,
            person(ANALYST, "SendRequiredActions"),
            &admin,
            Some(json!({ "actions": ["UPDATE_PASSWORD"] })),
            "USER",
        ),
        (
            "session_revoke",
            Method::POST,
            person(created_id, "RevokeSessions"),
            &admin,
            Some(json!({})),
            "USER",
        ),
        (
            "keys_rotate",
            Method::POST,
            format!("{BASE}/SigningKeys/Rotate"),
            &admin,
            Some(json!({})),
            "KEYS",
        ),
        (
            "consent_revoke",
            Method::POST,
            format!("{BASE}/Consents('consent-audit')/Revoke"),
            &admin,
            Some(json!({})),
            "CONSENT",
        ),
        (
            "outbox_retry",
            Method::POST,
            format!("{BASE}/EmailOutbox('outbox-audit')/Retry"),
            &admin,
            Some(json!({})),
            "EMAIL_OUTBOX",
        ),
        (
            "passkey_delete",
            Method::POST,
            format!("{BASE}/WebAuthnCredentials('pk-audit')/DeletePasskey"),
            &admin,
            Some(json!({})),
            "WEBAUTHN_CREDENTIAL",
        ),
        (
            "totp_enrol",
            Method::POST,
            person(ANALYST, "EnrolTotp"),
            &me,
            Some(json!({ "secret": totp_secret, "code": code })),
            "CREDENTIAL",
        ),
        (
            "recovery_codes_generate",
            Method::POST,
            person(ANALYST, "GenerateRecoveryCodes"),
            &me,
            Some(json!({})),
            "CREDENTIAL",
        ),
        (
            "person_delete",
            Method::DELETE,
            format!("{BASE}/People('{created_id}')"),
            &admin,
            None,
            "USER",
        ),
    ];
    let writes: Vec<&str> = OperationRegistry::new()
        .operations()
        .iter()
        .filter(|op| {
            matches!(op.odata.kind, ODataKind::Action | ODataKind::EntitySetWrite)
                && op.name != "allowed"
        })
        .map(|op| op.name)
        .collect();
    for name in &writes {
        assert!(
            NOT_IN_TABLE.contains(name) || table.iter().any(|row| row.0 == *name),
            "write operation {name} has no row in the audit table"
        );
    }
    for (name, method, uri, caller, body, resource) in table {
        let before = store.list_admin_events("dev", None).await.unwrap().len();
        let (st, _, _, raw) = api.call(method, &uri, caller, body).await;
        assert!(st.is_success(), "{name}: {st} {raw}");
        let events = store.list_admin_events("dev", None).await.unwrap();
        assert!(
            events[before..].iter().any(|e| e.resource_type == resource),
            "{name} left no {resource} admin event: {:?}",
            &events[before..]
        );
        assert!(
            !format!("{:?}", &events[before..]).contains("Audit-Pass-2026-long")
                && !format!("{:?}", &events[before..]).contains(totp_secret),
            "{name}: a secret reached the admin event"
        );
    }
}

#[tokio::test]
async fn self_service_actions_and_honest_not_implemented() {
    let api = Api::new().await;
    let me = api.token(ANALYST, json!({})).await;
    // EnrolTotp step 1: a secret, nothing stored.
    let uri = format!("{BASE}/People('{ANALYST}')/JoinedContext.Kaisatsu.EnrolTotp");
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
    let now = jc_time::TimestampTz::now().as_second() as u64;
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

    // K11 passkeys: step 1 hands out a server-made challenge; a challenge the caller made up is
    // refused before any credential is parsed (red before: any 16+ character string passed).
    let (st, _, opts, raw) = api
        .call(
            Method::POST,
            &format!("{BASE}/People('{ANALYST}')/RegisterPasskey"),
            &me,
            Some(json!({})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{raw}");
    let issued = opts["value"]["challenge"].as_str().unwrap().to_owned();
    let (st, _, err, _) = api
        .call(
            Method::POST,
            &format!("{BASE}/People('{ANALYST}')/RegisterPasskey"),
            &me,
            Some(json!({ "credential": {}, "challenge": "caller-chosen-challenge-value" })),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap()
            .contains("challenge"),
        "{err}"
    );
    // The issued challenge passes the challenge check and fails on the empty credential.
    let (st, _, err, _) = api
        .call(
            Method::POST,
            &format!("{BASE}/People('{ANALYST}')/RegisterPasskey"),
            &me,
            Some(json!({ "credential": {}, "challenge": issued })),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap()
            .contains("credential"),
        "{err}"
    );
    // The refused attempt spent its challenge: the same one is refused as used.
    let (st, _, err, _) = api
        .call(
            Method::POST,
            &format!("{BASE}/People('{ANALYST}')/RegisterPasskey"),
            &me,
            Some(json!({ "credential": {}, "challenge": issued })),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(err.to_string().contains("already used"), "{err}");

    // Step 2 with a signed attestation for the mount host stores the passkey, mints the person's
    // user handle and writes the admin event; the same credential id again is a 409.
    let register = |challenge: String, cred_id: &'static [u8]| {
        json!({
            "credential": passkey_registration(&challenge, "idm.dev.example.test", cred_id),
            "challenge": challenge,
        })
    };
    assert!(
        !api.store
            .get_user_by_id("dev", None, ANALYST)
            .await
            .unwrap()
            .unwrap()
            .attributes
            .contains_key("webauthn.user.handle")
    );
    let c = passkey_challenge(&api, &me, "idm.dev.example.test").await;
    let (st, _, row, raw) = api
        .call(
            Method::POST,
            &format!("{BASE}/People('{ANALYST}')/RegisterPasskey"),
            &me,
            Some(register(c, b"odata-passkey-1")),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{raw}");
    let stored = api
        .store
        .list_webauthn_credentials("dev", ANALYST)
        .await
        .unwrap();
    let pk = stored
        .iter()
        .find(|c| c.credential_id == b64url(b"odata-passkey-1"))
        .expect("passkey stored");
    assert_eq!(pk.user_label.as_deref(), Some("Passkey"));
    assert_eq!(pk.rp_id, "idm.dev.example.test");
    assert_eq!(pk.sign_count, 1);
    assert!(raw.contains(&pk.id), "{row}");
    let handle = api
        .store
        .get_user_by_id("dev", None, ANALYST)
        .await
        .unwrap()
        .unwrap()
        .attributes["webauthn.user.handle"][0]
        .clone();
    assert_eq!(pk.user_handle, handle);
    let c = passkey_challenge(&api, &me, "idm.dev.example.test").await;
    let (st, _, _, raw) = api
        .call(
            Method::POST,
            &format!("{BASE}/People('{ANALYST}')/RegisterPasskey"),
            &me,
            Some(register(c, b"odata-passkey-1")),
        )
        .await;
    assert_eq!(st, StatusCode::CONFLICT, "{raw}");

    // A realm with its own passkey rp id names it in the options and checks the attestation
    // against it; the second passkey keeps the handle the first one minted and its label.
    let mut realm = api.store.get_realm("dev").await.unwrap().unwrap();
    realm.web_authn_policy_passwordless_rp_id = " Example.TEST ".into();
    api.store.upsert_realm(realm).await.unwrap();
    let c = passkey_challenge(&api, &me, "example.test").await;
    let mut body = json!({
        "credential": passkey_registration(&c, "example.test", b"odata-passkey-2"),
        "challenge": c,
        "label": "Laptop",
    });
    let (st, _, _, raw) = api
        .call(
            Method::POST,
            &format!("{BASE}/People('{ANALYST}')/RegisterPasskey"),
            &me,
            Some(body.clone()),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{raw}");
    let stored = api
        .store
        .list_webauthn_credentials("dev", ANALYST)
        .await
        .unwrap();
    let pk2 = stored
        .iter()
        .find(|c| c.user_label.as_deref() == Some("Laptop"))
        .unwrap();
    assert_eq!(
        (pk2.rp_id.as_str(), pk2.user_handle.as_str()),
        ("example.test", handle.as_str())
    );
    // An attestation signed for another rp id is refused.
    let c = passkey_challenge(&api, &me, "example.test").await;
    body["credential"] = passkey_registration(&c, "idm.dev.example.test", b"odata-passkey-3");
    body["challenge"] = json!(c);
    let (st, _, _, raw) = api
        .call(
            Method::POST,
            &format!("{BASE}/People('{ANALYST}')/RegisterPasskey"),
            &me,
            Some(body),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{raw}");

    // DeletePasskey: a person deletes their own passkey; another person's answers 404 and stays
    // (red before: any caller in scope deleted any passkey).
    let steward = api
        .store
        .get_user_by_id("dev", None, STEWARD)
        .await
        .unwrap()
        .unwrap();
    let analyst = api
        .store
        .get_user_by_id("dev", None, ANALYST)
        .await
        .unwrap()
        .unwrap();
    for (id, user) in [("pk-steward", &steward), ("pk-analyst", &analyst)] {
        api.store
            .insert_webauthn_credential(WebAuthnCredentialRecord {
                id: id.into(),
                realm_id: "dev".into(),
                org_id: user.org_id.clone(),
                user_id: user.id.clone(),
                user_handle: format!("handle-{id}"),
                credential_id: format!("cred-{id}"),
                public_key_cose_json: "{}".into(),
                cose_alg: -7,
                sign_count: 0,
                uv_initialized: false,
                backup_eligible: false,
                backup_state: false,
                transports: Vec::new(),
                attestation_type: "none".into(),
                rp_id: "idm.dev.example.test".into(),
                user_label: None,
                created_at: jc_time::TimestampTz::now(),
                last_used_at: None,
            })
            .await
            .unwrap();
    }
    let (st, _, _, raw) = api
        .call(
            Method::POST,
            &format!("{BASE}/WebAuthnCredentials('pk-steward')/DeletePasskey"),
            &me,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{raw}");
    assert_eq!(
        api.store
            .list_webauthn_credentials("dev", STEWARD)
            .await
            .unwrap()
            .len(),
        1
    );
    let (st, _, _, raw) = api
        .call(
            Method::POST,
            &format!("{BASE}/WebAuthnCredentials('pk-analyst')/DeletePasskey"),
            &me,
            None,
        )
        .await;
    assert!(st.is_success(), "{st} {raw}");
    assert!(
        !api.store
            .list_webauthn_credentials("dev", ANALYST)
            .await
            .unwrap()
            .iter()
            .any(|c| c.id == "pk-analyst")
    );
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

/// Malformed function parameters, POST to an entity, paging and the export cap answer with the
/// reason a caller can act on.
#[tokio::test]
async fn odata_parameters_paging_and_export_cap() {
    let api = Api::new().await;
    let admin = api.admin().await;
    for (uri, needle) in [
        (
            format!("{BASE}/Allowed(operation)"),
            "malformed function parameters",
        ),
        (format!("{BASE}/Allowed(operation=abc)"), "bad literal abc"),
        (format!("{BASE}/Allowed(%20=1)"), "empty parameter name"),
    ] {
        let (st, _, err, raw) = api.call(Method::GET, &uri, &admin, None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{uri}: {raw}");
        assert!(err.to_string().contains(needle), "{uri}: {err}");
    }
    let (st, _, err, _) = api
        .call(
            Method::POST,
            &format!("{BASE}/People('{ANALYST}')"),
            &admin,
            Some(json!({ "username": "x@hel.fi" })),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(
        err.to_string().contains("POST goes to the collection"),
        "{err}"
    );

    // More people than one page: the page links the next one.
    for n in 0..5_001 {
        api.store
            .create_user(UserRepresentation {
                id: format!("bulk-{n}"),
                realm_id: "dev".into(),
                username: format!("bulk.{n}@example.test"),
                enabled: true,
                ..Default::default()
            })
            .await
            .unwrap();
    }
    let (st, _, page, raw) = api
        .call(Method::GET, &format!("{BASE}/People"), &admin, None)
        .await;
    assert_eq!(st, StatusCode::OK, "{raw}");
    let next = page["@odata.nextLink"].as_str().expect("next link");
    assert!(next.contains("/odata/People?$skiptoken="), "{next}");
    // The export stops at 5000 rows and says how to narrow it.
    let (st, _, err, _) = api
        .call(Method::GET, &format!("{BASE}/People/$export"), &admin, None)
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(err.to_string().contains("narrow it with $filter"), "{err}");
}

#[tokio::test]
async fn entity_sets_answer_queries_count_export_and_allowed_over_http() {
    let api = Api::new().await;
    let admin = api.admin().await;

    // $filter + $count on People.
    let (st, _, body, raw) = api
        .call(
            Method::GET,
            &format!("{BASE}/People?$filter=username%20eq%20'demo.analyst@hel.fi'&$count=true"),
            &admin,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{raw}");
    assert_eq!(body["@odata.count"], 1);
    assert_eq!(body["value"][0]["id"], ANALYST);
    assert!(body["value"][0].get("passwordHash").is_none());

    // Entity by key, and a missing key.
    let (st, _, row, _) = api
        .call(
            Method::GET,
            &format!("{BASE}/People('{ANALYST}')"),
            &admin,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(row["username"], "demo.analyst@hel.fi");
    let (st, _, err, _) = api
        .call(
            Method::GET,
            &format!("{BASE}/People('nobody')"),
            &admin,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(err["error"]["code"], "not-found");

    // $count and $export agree with the collection.
    let (_, _, all, _) = api
        .call(
            Method::GET,
            &format!("{BASE}/People?$count=true"),
            &admin,
            None,
        )
        .await;
    let total = all["@odata.count"].as_u64().unwrap();
    assert!(total >= 3);
    let (st, _, _, text) = api
        .call(Method::GET, &format!("{BASE}/People/$count"), &admin, None)
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(text.trim().parse::<u64>().unwrap(), total);
    let (st, _, exported, _) = api
        .call(Method::GET, &format!("{BASE}/People/$export"), &admin, None)
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(exported.as_array().unwrap().len() as u64, total);

    // $allowed on a visible key.
    let (st, _, dec, raw) = api
        .call(
            Method::GET,
            &format!("{BASE}/People/$allowed?op=read&key={ANALYST}"),
            &admin,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{raw}");
    assert_eq!(dec["allowed"], true);
    let (st, _, err, _) = api
        .call(
            Method::GET,
            &format!("{BASE}/People/$allowed?op=fly"),
            &admin,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(err["error"]["code"], "invalid-request");

    // Secret fields can be neither filtered nor ordered.
    for q in [
        "People?$filter=passwordHash%20ne%20null",
        "Credentials?$orderby=secretData",
    ] {
        let (st, _, err, raw) = api
            .call(Method::GET, &format!("{BASE}/{q}"), &admin, None)
            .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{q}: {raw}");
        assert_eq!(err["error"]["innererror"]["status"], 400);
    }

    // An action name is not GET as a function import; an unknown set is not found.
    let (st, _, _, _) = api
        .call(Method::GET, &format!("{BASE}/NoSuchSet"), &admin, None)
        .await;
    assert!(st == StatusCode::NOT_FOUND || st == StatusCode::BAD_REQUEST);

    // No token: an OData error, not a Keycloak or HTML page.
    let req = Request::builder()
        .uri(format!("{BASE}/People"))
        .body(Body::empty())
        .unwrap();
    let resp = api.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let err: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(err["error"]["code"].is_string());
}

#[tokio::test]
async fn mcp_lists_only_permitted_tools_and_lists_match_odata() {
    let api = Api::new().await;
    let admin = api.admin().await;
    let member = api
        .token(
            "de300000-0000-4000-8000-00000000000f",
            json!({ "jc_org": ["helsinki"] }),
        )
        .await;
    let rpc = |method: &str| json!({ "jsonrpc": "2.0", "id": 7, "method": method, "params": {} });

    let (st, _, init, _) = api
        .call(
            Method::POST,
            "/api/v1/realms/dev/mcp",
            &admin,
            Some(rpc("initialize")),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(init["id"], 7);
    assert_eq!(init["result"]["serverInfo"]["name"], "kaisatsu-mcp");

    let names = |body: &Value| -> Vec<String> {
        body["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect()
    };
    let (_, _, all, _) = api
        .call(
            Method::POST,
            "/api/v1/realms/dev/mcp",
            &admin,
            Some(rpc("tools/list")),
        )
        .await;
    let admin_tools = names(&all);
    assert_eq!(
        admin_tools.len(),
        OperationRegistry::new().operations().len()
    );
    assert!(all["result"]["tools"][0]["inputSchema"].is_object());
    let (_, _, few, _) = api
        .call(
            Method::POST,
            "/api/v1/realms/dev/mcp",
            &member,
            Some(rpc("tools/list")),
        )
        .await;
    let member_tools = names(&few);
    assert!(member_tools.len() < admin_tools.len());
    assert!(
        !member_tools
            .iter()
            .any(|t| t == "keys_rotate" || t == "realm_apply")
    );

    // A list tool and the OData set answer the same rows.
    let filter = "username eq 'demo.analyst@hel.fi'";
    let listed = mcp_value(
        &api.mcp(&admin, "wk_person_list", json!({ "filter": filter }))
            .await,
    );
    let (_, _, odata, _) = api
        .call(
            Method::GET,
            &format!("{BASE}/People?$filter=username%20eq%20'demo.analyst@hel.fi'"),
            &admin,
            None,
        )
        .await;
    let ids = |v: &Value| -> Vec<String> {
        let rows = v.get("value").unwrap_or(v);
        rows.as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(ids(&listed), ids(&odata));
    assert_eq!(ids(&listed), vec![ANALYST.to_owned()]);
    // Without a filter the list tool answers the whole set, as the OData set does.
    let all = mcp_value(&api.mcp(&admin, "wk_person_list", json!({})).await);
    let (_, _, every, _) = api
        .call(Method::GET, &format!("{BASE}/People"), &admin, None)
        .await;
    assert_eq!(ids(&all), ids(&every));
    assert!(ids(&all).contains(&ANALYST.to_owned()));

    // A member calling an admin tool gets the same slug OData answers.
    let refused = api.mcp(&member, "keys_rotate", json!({})).await;
    let (st, _, err, _) = api
        .call(
            Method::POST,
            &format!("{BASE}/SigningKeys/Rotate"),
            &member,
            Some(json!({})),
        )
        .await;
    assert!(st.is_client_error());
    assert_eq!(refused["error"]["data"]["slug"], err["error"]["code"]);
    assert_eq!(refused["error"]["data"]["status"], st.as_u16());

    // Unknown tool and unknown method are JSON-RPC errors.
    let unknown = api.mcp(&admin, "no_such_tool", json!({})).await;
    assert_eq!(unknown["error"]["code"], -32601);
    let (_, _, nm, _) = api
        .call(
            Method::POST,
            "/api/v1/realms/dev/mcp",
            &admin,
            Some(rpc("resources/list")),
        )
        .await;
    assert_eq!(nm["error"]["code"], -32601);

    // No token, no tools.
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/realms/dev/mcp")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(rpc("tools/list").to_string()))
        .unwrap();
    let resp = api.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
