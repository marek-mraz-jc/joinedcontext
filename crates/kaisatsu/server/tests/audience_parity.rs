#![forbid(unsafe_code)]
//! The `aud` of a workload token is the set Keycloak puts there for the same realm file: every
//! `oidc-audience-mapper` on the client or its default scopes (`included.client.audience` before
//! `included.custom.audience`), plus, from `oidc-audience-resolve-mapper`, every other client the
//! token's `resource_access` holds roles of. `keycloak_audiences` is that rule written from the
//! realm file alone, so a token carrying an audience no mapper or role puts there fails.
//!
//! One audience differs on purpose: [`NOT_ON_SERVICE_ACCOUNTS`]. Keycloak gives a new service
//! account the realm's default role, whose `account` client roles make audience resolve add
//! `account`, the audience of Keycloak's account REST. kaisatsu's service accounts hold no role
//! (the stores create them with the client), and kaisatsu's account REST takes the tokens of its
//! own `account-console` client: adding it would let every workload token call that REST.
//! Decided (jc-kaisatsu-audience 2026-11-09, owner may override).
//!
//! The fixture realm copies the shapes of prod's smartcity export (2026-11-08): a federated-jwt
//! workload with two custom audiences, a secret client naming a client audience, a secret client
//! with `fullScopeAllowed: false`. `every_workload_of_a_realm_export_gets_keycloak_audiences` runs
//! the same check over a whole realm file as the rehearsal applies it
//! (`KAISATSU_REALM_EXPORT=<home>/prod/state/realms/smartcity.json`, ignored by default: the file
//! is not in this repository).

use base64::engine::{Engine, general_purpose::URL_SAFE_NO_PAD};
use jc_router::Mount;
use kaisatsu_core::*;
use kaisatsu_crypto::*;
use kaisatsu_import::{ApplyMode, ApplyOptions, apply_realm_files};
use kaisatsu_oidc::OutboundGuard;
use kaisatsu_server::{KaisatsuState, router};
use kaisatsu_store_memory::MemoryStore;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

mod common;
use common::post_form;

const HOST: &str = "idm.example.test";
/// The audience Keycloak adds to a service account's token and kaisatsu does not (module doc).
const NOT_ON_SERVICE_ACCOUNTS: &str = "account";
const K8S_ISSUER: &str = "https://kubernetes.default.svc.cluster.local";

fn audience_mapper(key: &str, aud: &str) -> Value {
    json!({ "name": format!("audience-{aud}"), "protocol": "openid-connect",
            "protocolMapper": "oidc-audience-mapper",
            "config": { "id.token.claim": "false", "access.token.claim": "true",
                        "introspection.token.claim": "true", key: aud } })
}

fn fixture_realm() -> Value {
    let scopes = ["web-origins", "service_account", "acr", "profile", "roles", "basic", "email"];
    json!({
        "realm": "fx", "enabled": true,
        "defaultRole": { "name": "default-roles-fx", "composite": true },
        "roles": { "realm": [
            { "name": "offline_access" }, { "name": "uma_authorization" },
            { "name": "default-roles-fx", "composite": true,
              "composites": { "realm": ["offline_access", "uma_authorization"],
                              "client": { "account": ["manage-account", "view-profile"] } } }
        ], "client": { "account": [{ "name": "manage-account" }, { "name": "view-profile" }] } },
        "identityProviders": [{ "alias": "kubernetes", "providerId": "kubernetes", "enabled": true,
                                "config": { "issuer": K8S_ISSUER } }],
        "clientScopes": [
            { "name": "roles", "protocol": "openid-connect", "protocolMappers": [
                { "name": "audience resolve", "protocol": "openid-connect",
                  "protocolMapper": "oidc-audience-resolve-mapper",
                  "config": { "introspection.token.claim": "true", "access.token.claim": "true" } }] },
            { "name": "web-origins", "protocol": "openid-connect" },
            { "name": "service_account", "protocol": "openid-connect" },
            { "name": "acr", "protocol": "openid-connect" },
            { "name": "profile", "protocol": "openid-connect" },
            { "name": "basic", "protocol": "openid-connect" },
            { "name": "email", "protocol": "openid-connect" }
        ],
        "clients": [
            { "clientId": "account", "enabled": true, "publicClient": true },
            { "clientId": "portal-api", "enabled": true, "bearerOnly": true },
            { "clientId": "fx-pl-kataster", "enabled": true, "serviceAccountsEnabled": true,
              "clientAuthenticatorType": "federated-jwt", "fullScopeAllowed": true,
              "attributes": { "jwt.credential.issuer": "kubernetes",
                  "jwt.credential.sub": "system:serviceaccount:fx-pipeline-identities:pl-kataster" },
              "protocolMappers": [audience_mapper("included.custom.audience", "rpfqszwlbp5bonvg7kckhdsne5qedjor"),
                                  audience_mapper("included.custom.audience", "portal-internal")],
              "defaultClientScopes": scopes },
            { "clientId": "fx-hub", "enabled": true, "serviceAccountsEnabled": true,
              "clientAuthenticatorType": "client-secret", "secret": "$(env:FX_HUB)",
              "fullScopeAllowed": true,
              "protocolMappers": [audience_mapper("included.client.audience", "portal-api")],
              "defaultClientScopes": scopes },
            { "clientId": "fx-narrow", "enabled": true, "serviceAccountsEnabled": true,
              "clientAuthenticatorType": "client-secret", "secret": "$(env:FX_NARROW)",
              "fullScopeAllowed": false,
              "protocolMappers": [audience_mapper("included.custom.audience", "context-broker-main")],
              "defaultClientScopes": scopes }
        ]
    })
}

/// The client roles the service account of `client` holds: the realm's default role, expanded,
/// and what the realm file's `users` entry for it (`serviceAccountClientId`) maps.
fn service_account_client_roles(realm: &Value, client: &str) -> BTreeMap<String, BTreeSet<String>> {
    let default = realm["defaultRole"]["name"].as_str().unwrap_or_default();
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let users = realm["users"].as_array().cloned().unwrap_or_default();
    for user in users.iter().filter(|u| u["serviceAccountClientId"] == client) {
        for (other, names) in user["clientRoles"].as_object().into_iter().flatten() {
            for n in names.as_array().into_iter().flatten().filter_map(Value::as_str) {
                out.entry(other.clone()).or_default().insert(n.to_owned());
            }
        }
    }
    let roles = realm["roles"]["realm"].as_array().cloned().unwrap_or_default();
    if let Some(role) = roles.iter().find(|r| r["name"] == default)
        && let Some(by_client) = role["composites"]["client"].as_object()
    {
        for (client, names) in by_client {
            for n in names.as_array().into_iter().flatten().filter_map(Value::as_str) {
                out.entry(client.clone()).or_default().insert(n.to_owned());
            }
        }
    }
    out
}

/// Keycloak's `aud` for a `client_credentials` token of `client`, from the realm file alone.
fn keycloak_audiences(realm: &Value, client: &Value) -> BTreeSet<String> {
    let id = client["clientId"].as_str().unwrap_or_default();
    let scopes: Vec<&Value> = client["defaultClientScopes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|name| {
            realm["clientScopes"]
                .as_array()?
                .iter()
                .find(|s| s["name"] == *name)
        })
        .collect();
    let mappers: Vec<&Value> = scopes
        .iter()
        .flat_map(|s| s["protocolMappers"].as_array().into_iter().flatten())
        .chain(client["protocolMappers"].as_array().into_iter().flatten())
        .filter(|m| m["config"]["access.token.claim"].as_str().unwrap_or("true") == "true")
        .collect();
    let mut aud = BTreeSet::new();
    for m in &mappers {
        if m["protocolMapper"] == "oidc-audience-mapper" {
            let cfg = &m["config"];
            let value = [cfg["included.client.audience"].as_str(), cfg["included.custom.audience"].as_str()]
                .into_iter()
                .flatten()
                .find(|v| !v.is_empty());
            aud.extend(value.map(str::to_owned));
        }
    }
    let resolves = mappers.iter().any(|m| m["protocolMapper"] == "oidc-audience-resolve-mapper");
    let full_scope = client["fullScopeAllowed"].as_bool().unwrap_or(true);
    if resolves && full_scope {
        // With `fullScopeAllowed: false` and no scope mapping for this client, the token's
        // `resource_access` is empty, so audience resolve adds nothing.
        for (other, roles) in service_account_client_roles(realm, id) {
            if other != id && !roles.is_empty() {
                aud.insert(other);
            }
        }
    }
    aud
}

fn token_audiences(token: &str) -> BTreeSet<String> {
    let payload = token.split('.').nth(1).unwrap();
    let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap();
    match &claims["aud"] {
        Value::String(a) => BTreeSet::from([a.clone()]),
        Value::Array(list) => list.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
        _ => BTreeSet::new(),
    }
}

/// `text` with every `$(file:…/NAME)` turned into `$(env:NAME)` (a rehearsal's realm file names
/// its secret files), and a value for every `$(env:NAME)` / `$(NAME)` it then holds.
fn secret_values(text: &str) -> (String, BTreeMap<String, String>) {
    let mut out = BTreeMap::new();
    let mut doc = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("$(") {
        doc.push_str(&rest[..at]);
        rest = &rest[at + 2..];
        let Some(end) = rest.find(')') else { break };
        let inner = &rest[..end];
        let name = match inner.strip_prefix("file:") {
            Some(path) => path.rsplit('/').next().unwrap_or(path),
            None => inner.strip_prefix("env:").unwrap_or(inner),
        };
        doc.push_str(&format!("$(env:{name})"));
        out.insert(name.to_owned(), format!("value-of-{name}-Aa1!long-enough"));
        rest = &rest[end + 1..];
    }
    doc.push_str(rest);
    (doc, out)
}

/// Checks the token of every workload client of `realm`; returns the clients checked.
async fn check_every_workload(realm: Value) -> usize {
    let name = realm["realm"].as_str().unwrap().to_owned();
    let (text, secrets) = secret_values(&realm.to_string());
    let realm: Value = serde_json::from_str(&text).unwrap();
    let kek = Kek::generate_for_tests();
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new(kek.clone()));
    let opts = ApplyOptions {
        mode: ApplyMode::NoDelete,
        env_vars: secrets.clone(),
        ..Default::default()
    };
    apply_realm_files(&store, &[("realm.json".into(), realm.clone())], &opts)
        .await
        .expect("apply the realm");
    let k8s_kek = Kek::generate_for_tests();
    let k8s_key = KeyManager::generate_es256("k8s", &k8s_kek, 100, "ACTIVE").unwrap();
    let jwk: Value = serde_json::from_str(&k8s_key.public_jwk_json).unwrap();
    let state = KaisatsuState::new(store, kek, OutboundGuard::default(), Arc::new(LocalAuthorizer));
    state.guard().insert_mock(
        format!("{K8S_ISSUER}/openid/v1/jwks"),
        200,
        serde_json::to_vec(&json!({ "keys": [jwk] })).unwrap(),
    );
    let app = router(state, Mount::new("kaisatsu", "https", HOST, "").unwrap());
    let token_path = format!("/realms/{name}/protocol/openid-connect/token");
    let issuer = format!("https://{HOST}/realms/{name}");
    let now = chrono::Utc::now().timestamp();
    let mut checked = 0;
    let mut wrong = Vec::new();
    for client in realm["clients"].as_array().unwrap() {
        if client["serviceAccountsEnabled"] != true || client["enabled"] == false {
            continue;
        }
        let id = client["clientId"].as_str().unwrap();
        let assertion;
        let secret;
        let form: Vec<(&str, &str)> = if client["clientAuthenticatorType"] == "federated-jwt" {
            let sub = client["attributes"]["jwt.credential.sub"].as_str().unwrap();
            assertion = sign_jwt(&k8s_key, &k8s_kek, Some("JWT"), &json!({
                "iss": K8S_ISSUER, "sub": sub, "aud": issuer, "iat": now, "exp": now + 600 }))
            .unwrap();
            vec![
                ("grant_type", "client_credentials"),
                ("client_assertion_type", "urn:ietf:params:oauth:client-assertion-type:jwt-bearer"),
                ("client_assertion", &assertion),
            ]
        } else {
            let Some(raw) = client["secret"].as_str() else { continue };
            let var = raw.trim_start_matches("$(").trim_end_matches(')');
            secret = secrets
                .get(var.strip_prefix("env:").unwrap_or(var))
                .cloned()
                .unwrap_or_else(|| raw.to_owned());
            vec![
                ("grant_type", "client_credentials"),
                ("client_id", id),
                ("client_secret", &secret),
            ]
        };
        let (st, _, body) = post_form(&app, &token_path, &form, None).await;
        assert!(st.is_success(), "{id}: {st} {body}");
        let token: Value = serde_json::from_str(&body).unwrap();
        let got = token_audiences(token["access_token"].as_str().unwrap());
        let mut want = keycloak_audiences(&realm, client);
        want.remove(NOT_ON_SERVICE_ACCOUNTS);
        if got != want {
            wrong.push(format!("{id}: got {got:?}, Keycloak {want:?}"));
        }
        checked += 1;
    }
    assert!(wrong.is_empty(), "{} of {checked} differ:\n{}", wrong.len(), wrong.join("\n"));
    checked
}

#[tokio::test]
async fn a_workload_token_carries_exactly_the_audiences_keycloak_gives_it() {
    let realm = fixture_realm();
    // The rule above, spelled out for the fixture's three workloads.
    let clients = realm["clients"].as_array().unwrap();
    let want = |id: &str| keycloak_audiences(&realm, clients.iter().find(|c| c["clientId"] == id).unwrap());
    let set = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<BTreeSet<_>>();
    assert_eq!(want("fx-pl-kataster"), set(&["rpfqszwlbp5bonvg7kckhdsne5qedjor", "portal-internal", "account"]));
    assert_eq!(want("fx-hub"), set(&["portal-api", "account"]));
    assert_eq!(want("fx-narrow"), set(&["context-broker-main"]));
    assert_eq!(check_every_workload(realm.clone()).await, 3);
}

#[tokio::test]
#[ignore = "needs KAISATSU_REALM_EXPORT, a Keycloak realm export outside this repository"]
async fn every_workload_of_a_realm_export_gets_keycloak_audiences() {
    #[allow(clippy::disallowed_methods)] // A test input path, read once.
    let path = std::env::var("KAISATSU_REALM_EXPORT").expect("KAISATSU_REALM_EXPORT");
    let realm: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let checked = check_every_workload(realm).await;
    eprintln!("{checked} workload clients carry Keycloak's audiences");
}
