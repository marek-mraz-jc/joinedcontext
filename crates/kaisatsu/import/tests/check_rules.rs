#![forbid(unsafe_code)]
//! `kaisatsu check` and apply refusals (W1b): every dangling reference, policy rule and secret
//! rule names the JSON path it fails at; file merging; `$(file:...)` confinement.

use kaisatsu_core::Store;
use kaisatsu_import::*;
use kaisatsu_store_memory::MemoryStore;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

fn ri() -> Value {
    serde_json::from_str(include_str!("fixtures/ri_dev.json")).unwrap()
}

fn opts() -> ApplyOptions {
    ApplyOptions {
        mode: ApplyMode::NoDelete,
        dry_run: false,
        allow_literal_secrets: false,
        secrets_dir: None,
        env_vars: BTreeMap::new(),
    }
}

fn client_index(doc: &Value, id: &str) -> usize {
    doc["clients"]
        .as_array()
        .unwrap()
        .iter()
        .position(|c| c["clientId"] == id)
        .unwrap()
}

#[test]
fn every_reference_and_policy_rule_names_its_path() {
    let edge = client_index(&ri(), "edge");
    let ui = client_index(&ri(), "portal-ui");
    let api = client_index(&ri(), "portal-api");
    type Edit = Box<dyn Fn(&mut Value)>;
    let cases: Vec<(Edit, &str)> = vec![
        (
            Box::new(|d| {
                d.as_object_mut().unwrap().remove("realm");
            }),
            "$.realm is required",
        ),
        (
            Box::new(|d| {
                d["clientScopes"][0].as_object_mut().unwrap().remove("name");
            }),
            "$.clientScopes[0].name missing",
        ),
        (
            Box::new(|d| {
                d["clientScopes"][0]["protocolMappers"] =
                    json!([{"protocolMapper": "script-mapper"}])
            }),
            "unknown mapper kind 'script-mapper'",
        ),
        (
            Box::new(|d| d["defaultOptionalClientScopes"] = json!(["nope"])),
            "$.defaultOptionalClientScopes[0]: dangling",
        ),
        (
            Box::new(|d| {
                d["roles"]["realm"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("name");
            }),
            "$.roles.realm[0].name missing",
        ),
        (
            Box::new(|d| d["roles"]["realm"][0]["composites"] = json!({"realm": ["no-role"]})),
            "unknown role 'no-role'",
        ),
        (
            Box::new(|d| d["defaultGroups"] = json!(["/no-group"])),
            "unknown group '/no-group'",
        ),
        (
            Box::new(|d| {
                d["identityProviders"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("alias");
            }),
            "$.identityProviders[0].alias missing",
        ),
        (
            Box::new(|d| {
                d["smtpServer"] = json!({"host": "smtp", "from": "a@b.c", "password": "literal"})
            }),
            "$.smtpServer.password: literal secret",
        ),
        (
            Box::new(move |d| {
                d["clients"][edge]
                    .as_object_mut()
                    .unwrap()
                    .remove("clientId");
            }),
            "clientId missing",
        ),
        (
            Box::new(move |d| d["clients"][edge]["implicitFlowEnabled"] = json!(true)),
            "implicitFlowEnabled=true is forbidden",
        ),
        (
            Box::new(move |d| {
                d["clients"][ui]["attributes"]["pkce.code.challenge.method"] = json!("plain")
            }),
            "must require PKCE S256",
        ),
        (
            Box::new(move |d| {
                d["clients"][edge]["redirectUris"] = json!(["https://*.evil.example/cb"])
            }),
            "wildcard in host",
        ),
        (
            Box::new(move |d| d["clients"][edge]["secret"] = json!("literal-secret")),
            "secret: literal secret",
        ),
        (
            Box::new(move |d| {
                d["clients"][api]["attributes"]["jwt.credential.issuer"] = json!("no-idp")
            }),
            "dangling reference to IdP 'no-idp'",
        ),
        (
            Box::new(move |d| d["clients"][edge]["defaultClientScopes"] = json!(["no-scope"])),
            "unknown clientScope 'no-scope'",
        ),
        (
            Box::new(move |d| {
                d["clients"][edge]["protocolMappers"] = json!([{"protocolMapper": "x"}])
            }),
            "unknown mapper kind 'x'",
        ),
        (
            Box::new(|d| d["users"][0]["groups"] = json!(["/no-group"])),
            "$.users[0].groups[0]: dangling",
        ),
        (
            Box::new(|d| d["users"][0]["realmRoles"] = json!(["no-role"])),
            "unknown realm role 'no-role'",
        ),
        (
            Box::new(|d| d["users"][0]["credentials"][0]["value"] = json!("LiteralPass123!")),
            "$.users[0].credentials[0].value: literal secret",
        ),
        (
            Box::new(|d| d["authenticationFlows"][0]["alias"] = json!("my flow")),
            "unmappable authentication flow 'my flow'",
        ),
        (
            Box::new(|d| {
                d["authenticationFlows"][0]["authenticationExecutions"][0]["authenticator"] =
                    json!("script")
            }),
            "unknown authenticator 'script'",
        ),
        (
            Box::new(|d| {
                d["authenticationFlows"][0]["authenticationExecutions"][0]["authenticatorConfig"] =
                    json!("nope")
            }),
            "authenticatorConfig: dangling reference 'nope'",
        ),
        (
            Box::new(|d| d["displayName"] = json!("$(env:SNEAKY)")),
            "$.displayName: secret substitution is only allowed",
        ),
        (
            Box::new(|d| {
                d["clientScopes"][0]["attributes"] = json!({"x": ["$(file:/etc/passwd)"]})
            }),
            "secret substitution is only allowed",
        ),
    ];
    let base = ri();
    check_realm_value(&base, &opts()).unwrap();
    for (edit, want) in cases {
        let mut d = base.clone();
        edit(&mut d);
        let e = match check_realm_value(&d, &opts()) {
            Ok(w) => panic!("{want}: accepted, warnings {w:?}"),
            Err(e) => e.to_string(),
        };
        assert!(e.contains(want), "want {want}: {e}");
    }
    // Literal secrets pass where the operator allows them.
    let mut lit = base.clone();
    lit["clients"][edge]["secret"] = json!("literal-secret");
    let allow = ApplyOptions {
        allow_literal_secrets: true,
        ..opts()
    };
    check_realm_value(&lit, &allow).unwrap();
}

#[test]
fn merging_files_in_name_order() {
    let merged = merge_realm_files(&[
        ("20-more.json".into(), json!({"clients": [{"clientId": "b"}], "attributes": {"y": "2"}, "displayName": "later"})),
        ("10-base.json".into(), json!({"realm": "t", "clients": [{"clientId": "a"}], "attributes": {"x": "1"}, "displayName": "first"})),
    ])
    .unwrap();
    assert_eq!(
        merged["clients"],
        json!([{"clientId": "a"}, {"clientId": "b"}])
    );
    assert_eq!(merged["attributes"], json!({"x": "1", "y": "2"}));
    assert_eq!(merged["displayName"], "later");
    assert!(merge_realm_files(&[("a.json".into(), json!([1]))]).is_err());
}

#[tokio::test]
async fn secret_files_stay_inside_the_secrets_directory() {
    let base = std::env::temp_dir().join(format!("kaisatsu-secrets-{}", uuid::Uuid::now_v7()));
    let secrets = base.join("secrets");
    std::fs::create_dir_all(&secrets).unwrap();
    std::fs::write(secrets.join("edge"), "edge-from-file\n").unwrap();
    std::fs::write(base.join("outside"), "nope").unwrap();
    let edge = client_index(&ri(), "edge");
    let with_file = |path: String| {
        let mut d = ri();
        d["clients"][edge]["secret"] = json!(format!("$(file:{path})"));
        d
    };
    let in_dir = ApplyOptions {
        secrets_dir: Some(secrets.clone()),
        allow_literal_secrets: true,
        ..opts()
    };
    let refusals = [
        (
            with_file(secrets.join("edge").display().to_string()),
            opts(),
            "requires a configured secrets directory",
        ),
        (
            with_file(secrets.join("missing").display().to_string()),
            in_dir.clone(),
            "does not exist",
        ),
        (
            with_file(format!("{}/../outside", secrets.display())),
            in_dir.clone(),
            "escapes the configured secrets directory",
        ),
    ];
    for (doc, o, want) in refusals {
        let e = check_realm_value(&doc, &o).unwrap_err().to_string();
        assert!(e.contains(want), "{want}: {e}");
    }
    // Applied for real, the file's content becomes the client secret.
    let store = Arc::new(MemoryStore::default());
    let dyn_store: Arc<dyn Store> = store.clone();
    let mut doc = with_file(secrets.join("edge").display().to_string());
    // The other secrets come from literal values in this test.
    for u in doc["users"].as_array_mut().unwrap() {
        for c in u["credentials"].as_array_mut().unwrap() {
            if c.get("value").is_some() {
                c["value"] = json!("LiteralPass123!Valid");
            }
            if c.get("secretData").is_some() {
                c["secretData"] = json!(r#"{"value":"literal-seed-1234567890"}"#);
            }
        }
    }
    for c in doc["clients"].as_array_mut().unwrap() {
        if c["clientId"] != "edge" && c.get("secret").is_some() {
            c["secret"] = json!("literal");
        }
    }
    apply_realm_files(&dyn_store, &[("r.json".into(), doc)], &in_dir)
        .await
        .unwrap();
    let c = store
        .get_client_by_client_id("dev", "edge")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(c.secret.as_deref(), Some("edge-from-file"));
    std::fs::remove_dir_all(&base).unwrap();
}

#[tokio::test]
async fn a_literal_otp_seed_is_refused_like_any_literal_secret() {
    let mut doc = ri();
    let steward = doc["users"]
        .as_array()
        .unwrap()
        .iter()
        .position(|u| u["username"] == "demo.steward@hel.fi")
        .unwrap();
    for raw in [
        "plain-literal-seed",
        r#"{"value":"literal-in-json"}"#,
        r#"{"other":1}"#,
    ] {
        doc["users"][steward]["credentials"][1]["secretData"] = json!(raw);
        let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
        let o = ApplyOptions {
            env_vars: BTreeMap::from([
                (
                    "USER_PASSWORD_DEMO_STEWARD".into(),
                    "StewardPass123!Valid".into(),
                ),
                (
                    "USER_PASSWORD_DEMO_ORGMEMBER".into(),
                    "MemberPass123!Valid".into(),
                ),
                (
                    "USER_PASSWORD_DEMO_ANALYST".into(),
                    "AnalystPass123!Valid".into(),
                ),
                ("CLIENT_SECRET_EDGE".into(), "a".into()),
                ("CLIENT_SECRET_APISIX_GATEWAY".into(), "b".into()),
                ("CLIENT_SECRET_GRAFANA".into(), "c".into()),
                ("CLIENT_SECRET_GITEA".into(), "d".into()),
                ("CLIENT_SECRET_CKAN".into(), "e".into()),
            ]),
            ..opts()
        };
        let e = apply_realm_files(&store, &[("r.json".into(), doc.clone())], &o)
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("otp.secretData.value"), "{raw}: {e}");
        let allowed = ApplyOptions {
            allow_literal_secrets: true,
            ..o.clone()
        };
        let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
        apply_realm_files(&store, &[("r.json".into(), doc.clone())], &allowed)
            .await
            .unwrap();
    }
}

#[test]
fn the_client_authentication_flow_takes_the_methods_kaisatsu_implements() {
    let mut d = ri();
    let flows = d["authenticationFlows"].as_array_mut().unwrap();
    flows.push(json!({"alias": "clients", "topLevel": true, "builtIn": true,
        "providerId": "client-flow", "authenticationExecutions": [
            {"authenticator": "client-secret", "requirement": "ALTERNATIVE"},
            {"authenticator": "client-jwt", "requirement": "ALTERNATIVE"},
            {"authenticator": "federated-jwt", "requirement": "ALTERNATIVE"}]}));
    check_realm_value(&d, &opts()).unwrap();
    for unimplemented in ["client-secret-jwt", "client-x509"] {
        let mut e = d.clone();
        let last = e["authenticationFlows"].as_array().unwrap().len() - 1;
        e["authenticationFlows"][last]["authenticationExecutions"][0]["authenticator"] =
            json!(unimplemented);
        let err = check_realm_value(&e, &opts()).unwrap_err().to_string();
        assert!(err.contains(&format!("unknown authenticator '{unimplemented}'")), "{err}");
    }
}

#[test]
fn the_reset_credentials_flow_takes_the_steps_kaisatsu_implements() {
    let mut d = ri();
    d["authenticationFlows"].as_array_mut().unwrap().push(json!({"alias": "reset credentials",
        "topLevel": true, "builtIn": true, "providerId": "basic-flow", "authenticationExecutions": [
            {"authenticator": "reset-credentials-choose-user", "requirement": "REQUIRED"},
            {"authenticator": "reset-credential-email", "requirement": "REQUIRED"},
            {"authenticator": "reset-password", "requirement": "REQUIRED"}]}));
    check_realm_value(&d, &opts()).unwrap();
    let last = d["authenticationFlows"].as_array().unwrap().len() - 1;
    d["authenticationFlows"][last]["authenticationExecutions"][0]["authenticator"] =
        json!("reset-otp");
    let err = check_realm_value(&d, &opts()).unwrap_err().to_string();
    assert!(err.contains("unknown authenticator 'reset-otp'"), "{err}");
}
