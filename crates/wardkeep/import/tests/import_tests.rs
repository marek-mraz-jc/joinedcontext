#![forbid(unsafe_code)]
//! Tests for `wardkeep-import` (W1, W1b):
//! - Loads the deployment's RI (`tests/fixtures/ri_dev.json`) and every `keycloak-clients.yaml` client,
//!   verifying zero unknown fields and zero unknown mapper/authenticator kinds.
//! - Applies into an empty realm, verifies idempotency on second apply (`EXIT_NO_CHANGE`),
//!   verifies `--prune` (`ApplyMode::Full`) deletes only `managedBy: apply` objects and preserves
//!   Portal-created (`managed-by: joinedcontext`) objects.
//! - Verifies dangling reference refusal with JSON path, `$(file:)` outside secrets directory refusal,
//!   dry-run non-mutation, plan/apply with stale-plan refusal (`EXIT_STALE_OR_LOCK_TIMEOUT`), and
//!   export round-trip fixed point.

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use wardkeep_core::{ClientRepresentation, Store};
use wardkeep_import::*;
use wardkeep_store_memory::MemoryStore;

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

    // 1. Offline check (`wardkeep check`) passes with zero warnings (no unknown fields)
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
            .contains("wardkeep plan for realm `dev`")
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
        .upsert_realm(wardkeep_core::RealmConfig {
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
        wardkeep_core::WardkeepError::StalePlan(_)
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
