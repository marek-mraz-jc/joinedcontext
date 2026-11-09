#![forbid(unsafe_code)]
#![allow(clippy::disallowed_methods)] // Tests check the process environment holds what they assume.
//! Tests for `kaisatsu-import` (W1, W1b):
//! - Loads the deployment's RI (`tests/fixtures/ri_dev.json`) and every `keycloak-clients.yaml` client,
//!   verifying zero unknown fields and zero unknown mapper/authenticator kinds.
//! - Applies into an empty realm, verifies idempotency on second apply (`EXIT_NO_CHANGE`),
//!   verifies `--prune` (`ApplyMode::Full`) deletes only `managedBy: apply` objects and preserves
//!   Portal-created (`managed-by: joinedcontext`) objects.
//! - Verifies dangling reference refusal with JSON path, `$(file:)` outside secrets directory refusal,
//!   dry-run non-mutation, plan/apply with stale-plan refusal (`EXIT_STALE_OR_LOCK_TIMEOUT`), and
//!   export round-trip fixed point.

use kaisatsu_core::{ClientRepresentation, Store};
use kaisatsu_import::*;
use kaisatsu_store_memory::MemoryStore;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

fn ri_fixture() -> Value {
    let raw = include_str!("fixtures/ri_dev.json");
    serde_json::from_str(raw).expect("parse ri_dev.json")
}

fn default_opts() -> ApplyOptions {
    ApplyOptions {
        mode: ApplyMode::NoDelete,
        dry_run: false,
        allow_literal_secrets: false,
        secrets_dir: None,
        env_vars: BTreeMap::from([
            (
                "USER_PASSWORD_DEMO_STEWARD".into(),
                "StewardPass123!Valid".into(),
            ),
            (
                "USER_TOTP_DEMO_STEWARD".into(),
                "steward-totp-seed-123456".into(),
            ),
            (
                "USER_PASSWORD_DEMO_ORGMEMBER".into(),
                "MemberPass123!Valid".into(),
            ),
            (
                "USER_PASSWORD_DEMO_ANALYST".into(),
                "AnalystPass123!Valid".into(),
            ),
            ("CLIENT_SECRET_EDGE".into(), "edge-secret-123".into()),
            (
                "CLIENT_SECRET_APISIX_GATEWAY".into(),
                "apisix-secret-123".into(),
            ),
            ("CLIENT_SECRET_GRAFANA".into(), "grafana-secret-123".into()),
            ("CLIENT_SECRET_GITEA".into(), "gitea-secret-123".into()),
            ("CLIENT_SECRET_CKAN".into(), "ckan-secret-123".into()),
        ]),
    }
}

#[tokio::test]
async fn ri_and_all_keycloak_clients_apply_cleanly_and_second_apply_is_idempotent() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let doc = ri_fixture();
    let opts = default_opts();

    // 1. Offline check (`kaisatsu check`) passes with zero warnings (no unknown fields)
    let warnings = check_realm_value(&doc, &opts).expect("check_realm_value");
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

    // 2. Dry run writes nothing
    let mut dry_opts = opts.clone();
    dry_opts.dry_run = true;
    let dry_report = apply_realm_files(&store, &[("ri_dev.json".into(), doc.clone())], &dry_opts)
        .await
        .expect("dry run");
    assert!(dry_report.has_changes());
    assert!(store.get_realm("dev").await.unwrap().is_none());

    // 3. First real apply creates everything
    let report1 = apply_realm_files(&store, &[("ri_dev.json".into(), doc.clone())], &opts)
        .await
        .expect("first apply");
    assert_eq!(report1.exit_code(), EXIT_CHANGES_OR_APPLIED);
    assert!(
        report1
            .to_markdown()
            .contains("kaisatsu plan for realm `dev`")
    );

    // Verify fixed user IDs (PF-126) survived import
    let steward = store
        .get_user_by_id("dev", None, "de300000-0000-4000-8000-000000000001")
        .await
        .unwrap()
        .expect("demo.steward by fixed id");
    assert_eq!(steward.username, "demo.steward@hel.fi");

    // 4. Second apply of the same file is idempotent and reports all unchanged (exit code 0)
    let report2 = apply_realm_files(&store, &[("ri_dev.json".into(), doc.clone())], &opts)
        .await
        .expect("second apply");
    assert!(!report2.has_changes());
    assert_eq!(report2.exit_code(), EXIT_NO_CHANGE);
}

/// CIMD follows the realm's client policies, as in Keycloak: dev's `client-id-uri` policy gives
/// its domains; a realm file without it (prod's smartcity export) gets none, so no https
/// client_id signs in there.
#[tokio::test]
async fn cimd_domains_come_from_the_realm_client_policy_and_default_to_none() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    apply_realm_files(&store, &[("ri_dev.json".into(), ri_fixture())], &default_opts())
        .await
        .expect("dev applies");
    let dev = store.get_realm("dev").await.unwrap().unwrap();
    assert_eq!(dev.cimd_allowed_client_domains, ["claude.ai", "vscode.dev"]);
    assert!(dev.cimd_allowed_redirect_domains.iter().any(|d| d == "claude.ai"));

    let mut prod = ri_fixture();
    let map = prod.as_object_mut().unwrap();
    map.remove("clientPolicies");
    map.remove("clientProfiles");
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    apply_realm_files(&store, &[("prod.json".into(), prod)], &default_opts())
        .await
        .expect("a realm without the policy applies");
    let prod = store.get_realm("dev").await.unwrap().unwrap();
    assert!(prod.cimd_allowed_client_domains.is_empty(), "{:?}", prod.cimd_allowed_client_domains);
    assert!(prod.cimd_allowed_redirect_domains.is_empty());
}

#[tokio::test]
async fn prune_deletes_only_apply_managed_objects_and_preserves_portal_created_objects() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let mut doc = ri_fixture();
    let mut opts = default_opts();

    apply_realm_files(&store, &[("ri_dev.json".into(), doc.clone())], &opts)
        .await
        .unwrap();

    // Create a client in the Portal (`managed-by: joinedcontext`)
    store
        .upsert_client(ClientRepresentation {
            id: "portal-app-1".into(),
            realm_id: "dev".into(),
            client_id: "app-city-bikes".into(),
            managed_by: Some("joinedcontext".into()),
            attributes: BTreeMap::from([("managed-by".into(), "joinedcontext".into())]),
            ..Default::default()
        })
        .await
        .unwrap();

    // Remove `ckan` from the apply file and run with `--prune` (`ApplyMode::Full`)
    doc["clients"]
        .as_array_mut()
        .unwrap()
        .retain(|c| c["clientId"] != "ckan");
    opts.mode = ApplyMode::Full;

    let report = apply_realm_files(&store, &[("ri_dev.json".into(), doc)], &opts)
        .await
        .unwrap();
    assert!(
        report
            .changes
            .iter()
            .any(|c| c.kind == "Client" && c.id == "ckan" && c.action == ChangeAction::Delete)
    );

    // `ckan` (managedBy: apply) was pruned; `app-city-bikes` (managed-by: joinedcontext) is untouched!
    assert!(
        store
            .get_client_by_client_id("dev", "ckan")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_client_by_client_id("dev", "app-city-bikes")
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn dangling_reference_unknown_mapper_and_file_escape_fail_and_plan_refuses_stale_realm() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let opts = default_opts();

    // 1. Dangling scope reference fails with JSON path
    let bad_scope_doc = json!({
        "realm": "dev",
        "clients": [{
            "clientId": "test-client",
            "publicClient": false,
            "secret": "$(CLIENT_SECRET_EDGE)",
            "defaultClientScopes": ["non-existent-scope"]
        }]
    });
    let err = check_realm_value(&bad_scope_doc, &opts).unwrap_err();
    assert!(
        err.to_string()
            .contains("$.clients[0].defaultClientScopes[0]")
    );

    // 2. Unknown mapper kind fails
    let bad_mapper_doc = json!({
        "realm": "dev",
        "clients": [{
            "clientId": "test-client",
            "publicClient": false,
            "secret": "$(CLIENT_SECRET_EDGE)",
            "protocolMappers": [{
                "name": "bad",
                "protocolMapper": "oidc-javascript-script-mapper"
            }]
        }]
    });
    let err2 = check_realm_value(&bad_mapper_doc, &opts).unwrap_err();
    assert!(err2.to_string().contains("unknown mapper kind"));

    // 3. `$(file:/etc/passwd)` outside secrets directory is refused
    let mut file_opts = opts.clone();
    file_opts.secrets_dir = Some(std::env::temp_dir().join("wk-secrets-test"));
    let _ = std::fs::create_dir_all(file_opts.secrets_dir.as_ref().unwrap());
    let bad_file_doc = json!({
        "realm": "dev",
        "clients": [{
            "clientId": "test-client",
            "publicClient": false,
            "secret": "$(file:/etc/hosts)"
        }]
    });
    assert!(check_realm_value(&bad_file_doc, &file_opts).is_err());

    // 4. Plan / Apply with stale-plan refusal (exit 3 / StalePlan)
    let doc = ri_fixture();
    let plan = create_plan(&store, &[("ri_dev.json".into(), doc.clone())], &opts)
        .await
        .expect("create plan");

    // Mutate the realm in the server before applying the plan
    store
        .upsert_realm(kaisatsu_core::RealmConfig {
            id: "dev".into(),
            realm: "dev".into(),
            display_name: "Modified Before Apply".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    let stale_err = apply_plan(&store, &plan, &opts)
        .await
        .expect_err("stale plan must be refused");
    assert!(matches!(
        stale_err,
        kaisatsu_core::KaisatsuError::StalePlan(_)
    ));

    // 5. Export round-trip fixed point
    let store2: Arc<dyn Store> = Arc::new(MemoryStore::default());
    apply_realm_files(&store2, &[("ri_dev.json".into(), doc)], &opts)
        .await
        .unwrap();
    let exported1 = export_realm(&store2, "dev", false).await.unwrap();
    apply_realm_files(
        &store2,
        &[("exported.json".into(), exported1.clone())],
        &opts,
    )
    .await
    .unwrap();
    let exported2 = export_realm(&store2, "dev", false).await.unwrap();
    assert_eq!(exported1, exported2);
}

#[tokio::test]
async fn unset_secret_variables_never_become_guessable_secrets() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let var = "KAISATSU_IMPORT_TEST_UNSET_SECRET_7F3A";
    assert!(std::env::var(var).is_err());
    let doc = |secret: &str| {
        json!({
            "realm": "unset",
            // The realm defaults name these as default optional scopes; an export lists them.
            "clientScopes": [
                {"name": "connector", "protocol": "openid-connect"},
                {"name": "offline_access", "protocol": "openid-connect"}
            ],
            "clients": [{
                "clientId": "svc",
                "publicClient": false,
                "serviceAccountsEnabled": true,
                "secret": secret
            }]
        })
    };
    let opts = ApplyOptions::default();

    // The offline check and a dry run validate with the variable unset.
    assert!(check_realm_value(&doc(&format!("$(env:{var})")), &opts).is_ok());
    let dry = ApplyOptions {
        dry_run: true,
        ..Default::default()
    };
    apply_realm_files(
        &store,
        &[("a.json".into(), doc(&format!("$({var})")))],
        &dry,
    )
    .await
    .expect("dry run");

    // A real apply of a new client refuses, and writes nothing (all or nothing).
    let err = apply_realm_files(
        &store,
        &[("a.json".into(), doc(&format!("$(env:{var})")))],
        &opts,
    )
    .await
    .expect_err("unset variable refused");
    assert!(err.to_string().contains("$.clients[0].secret"), "{err}");
    assert!(store.get_realm("unset").await.unwrap().is_none());

    // With the variable given the client gets that secret.
    let with_var = ApplyOptions {
        env_vars: BTreeMap::from([(var.to_owned(), "the-real-secret".to_owned())]),
        ..Default::default()
    };
    apply_realm_files(
        &store,
        &[("a.json".into(), doc(&format!("$({var})")))],
        &with_var,
    )
    .await
    .expect("apply");
    let secret = || async {
        store
            .get_client_by_client_id("unset", "svc")
            .await
            .unwrap()
            .unwrap()
            .secret
    };
    assert_eq!(secret().await.as_deref(), Some("the-real-secret"));

    // Re-applying the files (secrets as variables) without the variable keeps the current
    // secret, as re-applying an export does.
    let mut reapply = doc(&format!("$(CLIENT_SECRET_SVC_{var})"));
    reapply["displayName"] = json!("changed so the checksum differs");
    apply_realm_files(&store, &[("export.json".into(), reapply)], &opts)
        .await
        .expect("re-apply export");
    assert_eq!(secret().await.as_deref(), Some("the-real-secret"));

    // A user's password variable and an SMTP password variable must be set.
    let user_doc = json!({
        "realm": "unset",
        "users": [{
            "username": "someone",
            "enabled": true,
            "credentials": [{"type": "password", "value": format!("$(env:{var})")}]
        }]
    });
    let err = apply_realm_files(&store, &[("u.json".into(), user_doc)], &opts)
        .await
        .expect_err("unset password variable refused");
    assert!(
        err.to_string().contains("$.users[0].credentials[0]"),
        "{err}"
    );
    let otp_doc = json!({
        "realm": "unset",
        "users": [{
            "username": "someone",
            "enabled": true,
            "credentials": [{
                "type": "otp",
                "secretData": json!({"value": format!("$({var})")}).to_string()
            }]
        }]
    });
    assert!(
        apply_realm_files(&store, &[("o.json".into(), otp_doc)], &opts)
            .await
            .is_err()
    );
    let smtp_doc = json!({
        "realm": "unset",
        "smtpServer": {"host": "mail", "from": "a@b.c", "password": format!("$(env:{var})")}
    });
    let err = apply_realm_files(&store, &[("s.json".into(), smtp_doc.clone())], &opts)
        .await
        .expect_err("unset SMTP password refused");
    assert!(err.to_string().contains("smtpServer.password"), "{err}");
    apply_realm_files(&store, &[("s.json".into(), smtp_doc.clone())], &with_var)
        .await
        .expect("apply SMTP");
    let smtp_password = || async {
        store
            .get_realm("unset")
            .await
            .unwrap()
            .unwrap()
            .smtp_server
            .unwrap()
            .password
    };
    assert_eq!(smtp_password().await, "the-real-secret");
    let mut again = smtp_doc;
    again["displayName"] = json!("again");
    apply_realm_files(&store, &[("s.json".into(), again)], &opts)
        .await
        .expect("an existing SMTP password is kept");
    assert_eq!(smtp_password().await, "the-real-secret");
}

/// Inside `jc` the importer reads only the values it is handed: a realm file naming another
/// module's variable (`PATH` stands for one; every process has it) resolves to nothing, and only
/// the standalone CLI's `with_process_env` reads the process environment.
#[tokio::test]
async fn a_realm_file_cannot_read_the_process_environment_unless_the_cli_asks() {
    assert!(std::env::var("PATH").is_ok());
    let doc = json!({
        "realm": "envread",
        "clients": [{
            "clientId": "svc",
            "publicClient": false,
            "serviceAccountsEnabled": true,
            "secret": "$(env:PATH)"
        }]
    });
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let err = apply_realm_files(
        &store,
        &[("a.json".into(), doc.clone())],
        &ApplyOptions::default(),
    )
    .await
    .expect_err("the process environment is not read");
    assert!(err.to_string().contains("$.clients[0].secret"), "{err}");
    apply_realm_files(
        &store,
        &[("a.json".into(), doc)],
        &ApplyOptions::default().with_process_env(),
    )
    .await
    .expect("the standalone CLI reads it");
}
