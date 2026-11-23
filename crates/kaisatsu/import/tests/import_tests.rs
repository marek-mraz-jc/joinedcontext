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

use kaisatsu_core::{ClientRepresentation, GroupRepresentation, Store};
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
        outbound_private_hosts: Vec::new(),
    }
}

#[tokio::test]
async fn ri_and_all_keycloak_clients_apply_cleanly_and_second_apply_is_idempotent() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let doc = ri_fixture();
    let opts = default_opts();

    // 1. Offline check (`kaisatsu check`) passes with no unknown field; the one warning names the
    // RI's `defaultGroups`, which kaisatsu stores but applies to nobody (S3-39).
    let warnings = check_realm_value(&doc, &opts).expect("check_realm_value");
    assert!(
        warnings.len() == 1 && warnings[0].starts_with("$.defaultGroups"),
        "unexpected warnings: {warnings:?}"
    );

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
    apply_realm_files(
        &store,
        &[("ri_dev.json".into(), ri_fixture())],
        &default_opts(),
    )
    .await
    .expect("dev applies");
    let dev = store.get_realm("dev").await.unwrap().unwrap();
    assert_eq!(dev.cimd_allowed_client_domains, ["claude.ai", "vscode.dev"]);
    assert!(
        dev.cimd_allowed_redirect_domains
            .iter()
            .any(|d| d == "claude.ai")
    );

    let mut prod = ri_fixture();
    let map = prod.as_object_mut().unwrap();
    map.remove("clientPolicies");
    map.remove("clientProfiles");
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    apply_realm_files(&store, &[("prod.json".into(), prod)], &default_opts())
        .await
        .expect("a realm without the policy applies");
    let prod = store.get_realm("dev").await.unwrap().unwrap();
    assert!(
        prod.cimd_allowed_client_domains.is_empty(),
        "{:?}",
        prod.cimd_allowed_client_domains
    );
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

/// An identity provider's upstream client secret resolves from its variable like a client's,
/// leaves the realm as a placeholder on export, and an export re-applied without the variable
/// keeps the stored secret.
#[tokio::test]
async fn an_identity_provider_secret_resolves_exports_as_a_placeholder_and_survives_reapply() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let doc = json!({
        "realm": "upstream",
        "enabled": true,
        "identityProviders": [{
            "alias": "entra-id",
            "providerId": "oidc",
            "enabled": true,
            "config": { "clientId": "kaisatsu", "clientSecret": "$(env:IDP_SECRET_ENTRA_ID)" }
        }]
    });
    let mut opts = default_opts();
    opts.env_vars
        .insert("IDP_SECRET_ENTRA_ID".into(), "upstream-secret-1".into());
    apply_realm_files(&store, &[("upstream.json".into(), doc)], &opts)
        .await
        .unwrap();
    let secret = |store: Arc<dyn Store>| async move {
        store
            .get_idp("upstream", "entra-id")
            .await
            .unwrap()
            .unwrap()
            .config["clientSecret"]
            .clone()
    };
    assert_eq!(secret(store.clone()).await, "upstream-secret-1");
    let exported = export_realm(&store, "upstream", false).await.unwrap();
    let text = exported.to_string();
    assert!(!text.contains("upstream-secret-1"), "{text}");
    assert!(text.contains("$(IDP_SECRET_ENTRA_ID)"), "{text}");
    // The export's identity providers alone, re-applied without the variable.
    let again = json!({
        "realm": "upstream",
        "enabled": true,
        "identityProviders": exported["identityProviders"],
    });
    apply_realm_files(&store, &[("exported.json".into(), again)], &default_opts())
        .await
        .unwrap();
    assert_eq!(secret(store.clone()).await, "upstream-secret-1");
}

/// A rotated variable behind an identity provider's or the realm's SMTP secret replaces the
/// stored one on the next apply; only an unset variable keeps it.
#[tokio::test]
async fn a_rotated_idp_or_smtp_secret_variable_replaces_the_stored_secret() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let doc = json!({
        "realm": "rotate",
        "smtpServer": {"host": "smtp.city.example", "from": "idm@city.example", "password": "$(env:SMTP_PASSWORD)"},
        "identityProviders": [{
            "alias": "entra-id", "providerId": "oidc", "enabled": true,
            "config": {"clientId": "kaisatsu", "clientSecret": "$(env:IDP_SECRET_ENTRA_ID)"}
        }]
    });
    let files = [("rotate.json".to_owned(), doc)];
    let opts = |v: &str| ApplyOptions {
        env_vars: BTreeMap::from([
            ("SMTP_PASSWORD".to_owned(), format!("smtp-{v}")),
            ("IDP_SECRET_ENTRA_ID".to_owned(), format!("idp-{v}")),
        ]),
        ..ApplyOptions::default()
    };
    let held = || async {
        let smtp = store
            .get_realm("rotate")
            .await
            .unwrap()
            .unwrap()
            .smtp_server
            .unwrap()
            .password;
        let idp = store
            .get_idp("rotate", "entra-id")
            .await
            .unwrap()
            .unwrap()
            .config["clientSecret"]
            .clone();
        (smtp, idp)
    };
    apply_realm_files(&store, &files, &opts("one"))
        .await
        .expect("first");
    assert_eq!(held().await, ("smtp-one".to_owned(), "idp-one".to_owned()));
    apply_realm_files(&store, &files, &opts("two"))
        .await
        .expect("rotated");
    assert_eq!(held().await, ("smtp-two".to_owned(), "idp-two".to_owned()));
    apply_realm_files(&store, &files, &ApplyOptions::default())
        .await
        .expect("unset");
    assert_eq!(held().await, ("smtp-two".to_owned(), "idp-two".to_owned()));
}

/// A re-applied person keeps the stored id and creation time whatever the file names; an
/// Organization the Portal made under another id keeps that id, and its new domains point at it.
#[tokio::test]
async fn a_reapply_keeps_stored_ids_of_people_and_portal_made_organizations() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let opts = ApplyOptions::default();
    let doc = |user: Value| json!({"realm": "ids", "users": [user]});
    apply_realm_files(
        &store,
        &[("r.json".into(), doc(json!({"username": "eva"})))],
        &opts,
    )
    .await
    .expect("first");
    let eva = store
        .get_user_by_username_or_email("ids", None, "eva")
        .await
        .unwrap()
        .unwrap();
    let forged =
        json!({"username": "eva", "id": "forged-id", "createdTimestamp": 1, "firstName": "Eva"});
    apply_realm_files(&store, &[("r.json".into(), doc(forged))], &opts)
        .await
        .expect("second");
    let again = store
        .get_user_by_username_or_email("ids", None, "eva")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(again.id, eva.id);
    assert_eq!(again.created_timestamp, eva.created_timestamp);
    assert_eq!(again.first_name.as_deref(), Some("Eva"));

    store
        .upsert_organization(kaisatsu_core::Organization {
            id: "org-made-in-portal".into(),
            realm_id: "ids".into(),
            name: "Espoo".into(),
            alias: "espoo".into(),
            enabled: true,
            description: String::new(),
            domains: Vec::new(),
            attributes: std::collections::HashMap::new(),
            managed_by: None,
        })
        .await
        .unwrap();
    let org = json!({"realm": "ids", "organizations": [{"name": "Espoo", "alias": "espoo", "domains": [{"name": "espoo.fi"}]}]});
    apply_realm_files(&store, &[("r.json".into(), org)], &opts)
        .await
        .expect("organization");
    let held = store
        .get_organization("ids", "espoo")
        .await
        .unwrap()
        .expect("espoo");
    assert_eq!(held.id, "org-made-in-portal");
    assert_eq!(held.domains.len(), 1);
    assert_eq!(held.domains[0].org_id, "org-made-in-portal");
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
    // T-7152: files that name no SMTP server leave the one the Portal's operators saved.
    let silent = json!({ "realm": "unset", "displayName": "silent" });
    apply_realm_files(&store, &[("s.json".into(), silent)], &opts)
        .await
        .expect("a file without smtpServer applies");
    assert_eq!(smtp_password().await, "the-real-secret");
    let kept = store.get_realm("unset").await.unwrap().unwrap();
    assert_eq!(kept.display_name, "silent");
    assert_eq!(kept.smtp_server.unwrap().host, "mail");
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

/// A secret file rotated behind an unchanged realm document reaches the stored client on the next
/// apply (a restart re-applies the same files); an apply with nothing rotated stays unchanged.
#[tokio::test]
async fn a_rotated_secret_file_behind_an_unchanged_document_is_applied() {
    let dir = std::env::temp_dir().join(format!("kaisatsu-rotate-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("SVC_SECRET");
    std::fs::write(&file, "first-secret-value").unwrap();
    let opts = ApplyOptions {
        secrets_dir: Some(dir.clone()),
        ..Default::default()
    };
    let doc = json!({
        "realm": "rotate",
        "clientScopes": [
            {"name": "connector", "protocol": "openid-connect"},
            {"name": "offline_access", "protocol": "openid-connect"}
        ],
        "clients": [{
            "clientId": "svc",
            "publicClient": false,
            "serviceAccountsEnabled": true,
            "secret": format!("$(file:{})", file.display())
        }]
    });
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let files = [("realm.json".to_owned(), doc)];
    let secret = || async {
        store
            .get_client_by_client_id("rotate", "svc")
            .await
            .unwrap()
            .unwrap()
            .secret
    };
    apply_realm_files(&store, &files, &opts)
        .await
        .expect("apply");
    assert_eq!(secret().await.as_deref(), Some("first-secret-value"));
    let again = apply_realm_files(&store, &files, &opts)
        .await
        .expect("re-apply");
    assert!(!again.has_changes(), "nothing rotated: unchanged");

    std::fs::write(&file, "rotated-secret-value").unwrap();
    let rotated = apply_realm_files(&store, &files, &opts)
        .await
        .expect("apply rotated");
    assert_ne!(rotated.checksum, again.checksum);
    assert_eq!(secret().await.as_deref(), Some("rotated-secret-value"));
    // The checksum holds no secret value.
    assert!(!rotated.checksum.contains("rotated"));
    std::fs::remove_dir_all(&dir).ok();
}

/// jc substitutes each Organization realm's derived client secret as `$(NAME)` (T-7267): a
/// restart with a changed value stores it and reports the realm changed once (T-7288, where the
/// client diff left the secret out and the start logged "changed: false"); the next restart is
/// unchanged. Values are compared as hashes, never printed.
#[tokio::test]
async fn a_changed_secret_variable_is_applied_and_reported_once() {
    let doc = json!({
        "realm": "derived",
        "clients": [{
            "clientId": "portal-reconciler",
            "publicClient": false,
            "serviceAccountsEnabled": true,
            "secret": "$(CLIENT_SECRET_PORTAL_RECONCILER)"
        }]
    });
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let files = [("realm.json".to_owned(), doc)];
    let hash = |s: &str| <sha2::Sha256 as sha2::Digest>::digest(s).to_vec();
    let opts = |value: &str| ApplyOptions {
        env_vars: [(
            "CLIENT_SECRET_PORTAL_RECONCILER".to_owned(),
            value.to_owned(),
        )]
        .into(),
        ..Default::default()
    };
    let stored = || async {
        let s = store
            .get_client_by_client_id("derived", "portal-reconciler")
            .await
            .unwrap()
            .unwrap()
            .secret
            .unwrap_or_default();
        hash(&s)
    };
    apply_realm_files(&store, &files, &opts("installation-secret-a"))
        .await
        .expect("apply A");
    assert_eq!(stored().await, hash("installation-secret-a"));

    let derived = apply_realm_files(&store, &files, &opts("derived-secret-b"))
        .await
        .expect("restart with B");
    assert_eq!(stored().await, hash("derived-secret-b"), "B is stored");
    assert!(
        derived.has_changes(),
        "a changed secret reports the realm changed"
    );

    let again = apply_realm_files(&store, &files, &opts("derived-secret-b"))
        .await
        .expect("restart with B again");
    assert!(!again.has_changes(), "nothing changed: unchanged");
    assert_eq!(stored().await, hash("derived-secret-b"));
}

#[test]
fn the_markdown_report_counts_each_action_and_lists_warnings_only_when_there_are_some() {
    let change = |action: ChangeAction| ObjectChange {
        kind: "Client".into(),
        id: "c".into(),
        action,
        changed_fields: Vec::new(),
    };
    let mut report = ApplyReport {
        realm: "md".into(),
        dry_run: true,
        checksum: String::new(),
        base_state_checksum: String::new(),
        changes: [
            vec![change(ChangeAction::Create); 1],
            vec![change(ChangeAction::Update); 2],
            vec![change(ChangeAction::Delete); 3],
            vec![change(ChangeAction::Unchanged); 4],
        ]
        .concat(),
        warnings: Vec::new(),
    };
    let md = report.to_markdown();
    assert_eq!(
        md,
        "### kaisatsu plan for realm `md` (dry-run)\n\n| Action | Count |\n|---|---:|\n\
         | Create | 1 |\n| Update | 2 |\n| Delete | 3 |\n| Unchanged | 4 |\n"
    );
    report.warnings.push("$.defaultGroups is stored".into());
    assert!(
        report
            .to_markdown()
            .ends_with("\n#### Warnings\n- $.defaultGroups is stored\n"),
        "{}",
        report.to_markdown()
    );
}

/// Only `$(...)` references enter the fast path's checksum: a plain value with parentheses
/// reads the same whether or not literal secrets are allowed, so the checksum holds and the
/// second apply is the fast path's single unchanged realm.
#[tokio::test]
async fn plain_text_with_parentheses_is_no_secret_reference_in_the_checksum() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let files = [(
        "r.json".to_owned(),
        json!({"realm": "ck", "displayName": "Roads (north)", "displayNameHtml": "$(not closed"}),
    )];
    let first = apply_realm_files(&store, &files, &ApplyOptions::default())
        .await
        .expect("first apply");
    let literal = ApplyOptions {
        allow_literal_secrets: true,
        ..ApplyOptions::default()
    };
    let again = apply_realm_files(&store, &files, &literal)
        .await
        .expect("second apply");
    assert_eq!(again.changes.len(), 1, "{:?}", again.changes);
    assert_eq!(again.changes[0].kind, "Realm");
    assert_eq!(again.changes[0].action, ChangeAction::Unchanged);
    assert_eq!(again.checksum, first.checksum);
}

/// `--prune` also deletes the client scopes, realm roles, clients and client roles a file
/// stopped naming; a dry run of a client the file adds plans its roles without writing them.
#[tokio::test]
async fn prune_deletes_scopes_roles_clients_and_client_roles_the_file_dropped() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let first = json!({
        "realm": "prune",
        "clientScopes": [{"name": "gone-scope", "protocol": "openid-connect"}],
        "roles": {"realm": [{"name": "gone-role"}], "client": {"keep": [{"name": "gone-client-role"}]}},
        "clients": [{"clientId": "keep", "publicClient": true}, {"clientId": "gone", "publicClient": true}]
    });
    let opts = ApplyOptions::default();
    apply_realm_files(&store, &[("realm.json".into(), first)], &opts)
        .await
        .expect("first apply");
    let keep = store
        .get_client_by_client_id("prune", "keep")
        .await
        .unwrap()
        .expect("client keep");
    assert_eq!(
        store
            .list_roles("prune", Some(&keep.id))
            .await
            .unwrap()
            .len(),
        1
    );

    let second = json!({
        "realm": "prune",
        "roles": {"client": {"keep": []}},
        "clients": [{"clientId": "keep", "publicClient": true}]
    });
    let full = ApplyOptions {
        mode: ApplyMode::Full,
        ..ApplyOptions::default()
    };
    // A dry run plans the same deletes and keeps everything.
    let dry_full = ApplyOptions {
        dry_run: true,
        ..full.clone()
    };
    let planned = apply_realm_files(&store, &[("realm.json".into(), second.clone())], &dry_full)
        .await
        .expect("prune dry run");
    assert_eq!(
        planned
            .changes
            .iter()
            .filter(|c| c.action == ChangeAction::Delete)
            .count(),
        4,
        "{:?}",
        planned.changes
    );
    assert!(
        store
            .get_client_by_client_id("prune", "gone")
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        store
            .list_roles("prune", Some(&keep.id))
            .await
            .unwrap()
            .len(),
        1
    );
    let report = apply_realm_files(&store, &[("realm.json".into(), second)], &full)
        .await
        .expect("prune apply");
    let deleted: Vec<(String, String)> = report
        .changes
        .iter()
        .filter(|c| c.action == ChangeAction::Delete)
        .map(|c| (c.kind.clone(), c.id.clone()))
        .collect();
    for (kind, id) in [
        ("ClientScope", "gone-scope"),
        ("Role", "gone-role"),
        ("Client", "gone"),
        ("ClientRole", "keep/gone-client-role"),
    ] {
        assert!(
            deleted.contains(&(kind.into(), id.into())),
            "{kind} {id}: {deleted:?}"
        );
    }
    assert!(
        store
            .list_client_scopes("prune")
            .await
            .unwrap()
            .iter()
            .all(|s| s.name != "gone-scope")
    );
    assert!(
        store
            .list_roles("prune", None)
            .await
            .unwrap()
            .iter()
            .all(|r| r.name != "gone-role")
    );
    assert!(
        store
            .get_client_by_client_id("prune", "gone")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .list_roles("prune", Some(&keep.id))
            .await
            .unwrap()
            .is_empty()
    );

    // A dry run plans the roles of a client only the file names, and writes neither.
    let added = json!({
        "realm": "prune",
        "roles": {"client": {"fresh": [{"name": "viewer"}]}},
        "clients": [{"clientId": "keep", "publicClient": true}, {"clientId": "fresh", "publicClient": true}]
    });
    let dry = ApplyOptions {
        dry_run: true,
        ..ApplyOptions::default()
    };
    let plan = apply_realm_files(&store, &[("realm.json".into(), added)], &dry)
        .await
        .expect("dry run");
    assert!(
        plan.changes.iter().any(|c| c.kind == "ClientRole"
            && c.id == "fresh/viewer"
            && c.action == ChangeAction::Create),
        "{:?}",
        plan.changes
    );
    assert!(
        store
            .get_client_by_client_id("prune", "fresh")
            .await
            .unwrap()
            .is_none()
    );
}

/// Organizations: the alias defaults to the name, a name is required, attributes must be a map,
/// and a domain the file adds later joins the stored ones unverified.
#[tokio::test]
async fn organizations_default_their_alias_and_add_new_domains_unverified() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let opts = ApplyOptions::default();
    let org = |domains: Value| json!({"realm": "orgs", "organizations": [{"name": "Helsinki", "domains": domains}]});
    apply_realm_files(
        &store,
        &[("r.json".into(), org(json!([{"name": "hel.fi"}])))],
        &opts,
    )
    .await
    .expect("first apply");
    let held = store
        .get_organization("orgs", "Helsinki")
        .await
        .unwrap()
        .expect("alias is the name");
    assert_eq!(held.domains.len(), 1);
    apply_realm_files(
        &store,
        &[(
            "r.json".into(),
            org(json!([{"name": "hel.fi"}, {"name": "espoo.fi"}])),
        )],
        &opts,
    )
    .await
    .expect("second apply");
    let held = store
        .get_organization("orgs", "Helsinki")
        .await
        .unwrap()
        .unwrap();
    let names: Vec<&str> = held.domains.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["hel.fi", "espoo.fi"]);
    assert!(
        held.domains
            .iter()
            .all(|d| !d.verified && d.org_id == held.id)
    );

    for (doc, want) in [
        (
            json!({"realm": "orgs", "organizations": [{"alias": "x"}]}),
            "$.organizations[].name is required",
        ),
        (
            json!({"realm": "orgs", "organizations": [{"name": "Espoo", "attributes": "flat"}]}),
            "$.organizations[Espoo].attributes",
        ),
    ] {
        let err = apply_realm_files(&store, &[("r.json".into(), doc)], &opts)
            .await
            .expect_err(want);
        assert!(
            matches!(err, kaisatsu_core::KaisatsuError::BadRequest(_)),
            "{err}"
        );
        assert!(err.to_string().contains(want), "{err}");
    }
}

/// A client role or required action of the wrong type is refused naming its section; a required
/// action keeps the id the file gives; a user keeps the `managedBy` the file gives.
#[tokio::test]
async fn client_roles_required_actions_and_users_are_read_as_the_file_names_them() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let opts = ApplyOptions {
        allow_literal_secrets: true,
        ..ApplyOptions::default()
    };
    let doc = json!({
        "realm": "ra",
        "requiredActions": [{"alias": "VERIFY_EMAIL", "id": "ra-verify", "name": "Verify e-mail"}],
        "users": [{"username": "portal-made", "managedBy": "joinedcontext"}]
    });
    apply_realm_files(&store, &[("r.json".into(), doc)], &opts)
        .await
        .expect("apply");
    let actions = store.list_required_actions("ra").await.unwrap();
    assert_eq!(actions[0].id, "ra-verify");
    let user = store
        .get_user_by_username_or_email("ra", None, "portal-made")
        .await
        .unwrap()
        .expect("user");
    assert_eq!(user.managed_by.as_deref(), Some("joinedcontext"));

    for (doc, want) in [
        (
            json!({"realm": "ra", "clients": [{"clientId": "c", "publicClient": true}],
                   "roles": {"client": {"c": [{"name": "r", "composite": "yes"}]}}}),
            "$.roles.client.c",
        ),
        (
            json!({"realm": "ra", "requiredActions": [{"alias": "A", "enabled": "yes"}]}),
            "$.requiredActions",
        ),
    ] {
        let err = apply_realm_files(&store, &[("r.json".into(), doc)], &opts)
            .await
            .expect_err(want);
        assert!(err.to_string().contains(want), "{err}");
    }
}

/// A migrated Keycloak password hash (`secretData` + `credentialData`) is stored as given; a
/// password entry with neither a value nor a hash and a credential of another type store nothing.
/// A group named without its leading slash links like its path.
#[tokio::test]
async fn a_migrated_password_hash_is_stored_and_empty_credentials_store_nothing() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let hash = r#"{"value":"aGFzaA==","salt":"c2FsdA=="}"#;
    let algo = r#"{"hashIterations":27500,"algorithm":"pbkdf2-sha256"}"#;
    let doc = json!({
        "realm": "creds",
        "groups": [{"name": "team", "path": "/team", "subGroups": [{"name": "night"}]}],
        "users": [
            {"username": "migrated", "groups": ["team", "/team/night"],
             "credentials": [{"type": "password", "secretData": hash, "credentialData": algo, "userLabel": "old"}]},
            {"username": "empty", "credentials": [{"type": "password"}, {"type": "webauthn"}]}
        ]
    });
    apply_realm_files(&store, &[("r.json".into(), doc)], &ApplyOptions::default())
        .await
        .expect("apply");
    let user = |name: &'static str| {
        let store = store.clone();
        async move {
            store
                .get_user_by_username_or_email("creds", None, name)
                .await
                .unwrap()
                .expect("user")
        }
    };
    let migrated = user("migrated").await;
    let creds = store.list_credentials("creds", &migrated.id).await.unwrap();
    assert_eq!(creds.len(), 1);
    assert_eq!(creds[0].credential_type, "password");
    assert_eq!(creds[0].secret_data, hash);
    assert_eq!(creds[0].credential_data, algo);
    assert_eq!(creds[0].user_label.as_deref(), Some("old"));
    let groups = store.list_user_groups("creds", &migrated.id).await.unwrap();
    let mut paths: Vec<&str> = groups.iter().map(|g| g.path.as_str()).collect();
    paths.sort_unstable();
    assert_eq!(paths, ["/team", "/team/night"]);
    let empty = user("empty").await;
    assert!(
        store
            .list_credentials("creds", &empty.id)
            .await
            .unwrap()
            .is_empty()
    );
}

/// Re-applying a person keeps the client roles an administrator mapped, merges the file's
/// attributes into stored ones the person had none of, and re-creates a group the Portal deleted.
#[tokio::test]
async fn a_reapply_keeps_admin_client_roles_and_recreates_a_deleted_group() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let opts = ApplyOptions::default();
    let doc = |attrs: Value| {
        let mut ann = json!({"username": "ann"});
        if !attrs.is_null() {
            ann["attributes"] = attrs;
        }
        json!({
            "realm": "keep",
            "clients": [{"clientId": "app", "publicClient": true}],
            "roles": {"client": {"app": [{"name": "viewer"}, {"name": "editor"}]}},
            "groups": [{"name": "crew", "path": "/crew"}],
            "users": [ann]
        })
    };
    apply_realm_files(&store, &[("r.json".into(), doc(json!(null)))], &opts)
        .await
        .expect("first apply");
    let ann = store
        .get_user_by_username_or_email("keep", None, "ann")
        .await
        .unwrap()
        .expect("ann");
    // A client the person names without a role_mappings row yields no administrator grant.
    let unmapped = kaisatsu_core::UserRepresentation {
        client_roles: std::collections::HashMap::from([(
            "app".to_owned(),
            vec!["viewer".to_owned()],
        )]),
        ..ann.clone()
    };
    assert!(
        admin_grants(&store, "keep", &unmapped)
            .await
            .unwrap()
            .1
            .is_empty()
    );

    // An administrator maps the client role `editor` (a role_mappings row and the user's grants).
    let app = store
        .get_client_by_client_id("keep", "app")
        .await
        .unwrap()
        .expect("app");
    let editor = store
        .list_roles("keep", Some(&app.id))
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.name == "editor")
        .expect("editor");
    store
        .add_role_mapping(kaisatsu_core::RoleMappingRecord {
            id: "rm-ann-editor".into(),
            realm_id: "keep".into(),
            org_id: None,
            principal_kind: "user".into(),
            principal_id: ann.id.clone(),
            role_id: editor.id.clone(),
            role_name: "editor".into(),
            client_uuid: Some(app.id.clone()),
        })
        .await
        .unwrap();
    let grants_by_admin =
        std::collections::HashMap::from([("app".to_owned(), vec!["editor".to_owned()])]);
    store
        .set_user_grants("keep", &ann.id, &[], &grants_by_admin)
        .await
        .unwrap();
    let held = store
        .get_user_by_username_or_email("keep", None, "ann")
        .await
        .unwrap()
        .unwrap();
    let grants = admin_grants(&store, "keep", &held).await.unwrap();
    assert_eq!(grants.1["app"], ["editor"]);

    let crew = store
        .get_group_by_path("keep", None, "/crew")
        .await
        .unwrap()
        .expect("crew");
    store.delete_group("keep", None, &crew.id).await.unwrap();
    let report = apply_realm_files(
        &store,
        &[("r.json".into(), doc(json!({"dept": ["roads"]})))],
        &opts,
    )
    .await
    .expect("re-apply");
    assert!(report.has_changes());
    assert!(
        store
            .get_group_by_path("keep", None, "/crew")
            .await
            .unwrap()
            .is_some()
    );
    let ann = store
        .get_user_by_username_or_email("keep", None, "ann")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ann.client_roles["app"], ["editor"]);
    assert_eq!(ann.attributes["dept"], ["roads"]);

    // The same file once more, the group deleted again: the fast path is not taken.
    let crew = store
        .get_group_by_path("keep", None, "/crew")
        .await
        .unwrap()
        .unwrap();
    store.delete_group("keep", None, &crew.id).await.unwrap();
    let again = apply_realm_files(
        &store,
        &[("r.json".into(), doc(json!({"dept": ["roads"]})))],
        &opts,
    )
    .await
    .expect("third apply");
    assert!(again.has_changes(), "a deleted group forces a real apply");
    assert!(
        store
            .get_group_by_path("keep", None, "/crew")
            .await
            .unwrap()
            .is_some()
    );
}

/// The export replaces a pairwise mapper's salt with a variable reference, like every secret.
#[tokio::test]
async fn the_export_names_a_pairwise_salt_by_variable() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let doc = json!({
        "realm": "salt",
        "clients": [{"clientId": "city-app", "publicClient": true, "protocolMappers": [{
            "name": "pairwise", "protocol": "openid-connect",
            "protocolMapper": "oidc-sha256-pairwise-sub-mapper",
            "config": {"pairwiseSubAlgorithmSalt": "literal-salt-value"}
        }]}]
    });
    let opts = ApplyOptions {
        allow_literal_secrets: true,
        ..ApplyOptions::default()
    };
    apply_realm_files(&store, &[("r.json".into(), doc)], &opts)
        .await
        .expect("apply");
    let out = export_realm(&store, "salt", false).await.expect("export");
    let text = out.to_string();
    assert!(!text.contains("literal-salt-value"), "{text}");
    let mapper = &out["clients"][0]["protocolMappers"][0];
    assert_eq!(
        mapper["config"]["pairwiseSubAlgorithmSalt"],
        "$(PAIRWISE_SALT_CITY_APP)"
    );
}

/// The Portal deletes a `managed-by: joinedcontext` group while its repository is still empty and
/// makes it again from the manifest without its client roles (prod rehearsal gate 4): the next
/// start re-applies the unchanged file and gives the group its roles back; a role added since
/// leaves the fast path alone.
#[tokio::test]
async fn a_group_that_lost_its_roles_behind_an_unchanged_file_gets_them_back() {
    let doc = json!({
        "realm": "groups",
        "clients": [{"clientId": "app-skoly", "publicClient": true}],
        "groups": [{
            "name": "skoly-viewer", "path": "/skoly-viewer",
            "attributes": {"managed-by": ["joinedcontext"]},
            "clientRoles": {"app-skoly": ["viewer"]},
            "subGroups": [{"name": "inner", "path": "/skoly-viewer/inner", "realmRoles": ["reader"]}]
        }],
        "roles": {"realm": [{"name": "reader"}]}
    });
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let files = [("realm.json".to_owned(), doc)];
    let opts = ApplyOptions::default();
    apply_realm_files(&store, &files, &opts)
        .await
        .expect("apply");
    let group = || async {
        store
            .get_group_by_path("groups", None, "/skoly-viewer")
            .await
            .unwrap()
            .expect("the group")
    };
    assert_eq!(group().await.client_roles["app-skoly"], ["viewer"]);

    // The Portal's delete and re-create: same path, a new id, no roles.
    let old = group().await;
    store.delete_group("groups", None, &old.id).await.unwrap();
    store
        .upsert_group(GroupRepresentation {
            realm_id: "groups".into(),
            name: "skoly-viewer".into(),
            path: "/skoly-viewer".into(),
            managed_by: Some("joinedcontext".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(group().await.client_roles.is_empty());

    let again = apply_realm_files(&store, &files, &opts)
        .await
        .expect("re-apply");
    assert!(again.has_changes(), "the lost roles force a real apply");
    assert_eq!(group().await.client_roles["app-skoly"], ["viewer"]);

    // A client role the Portal added since: still the fast path.
    let mut held = group().await;
    held.client_roles
        .get_mut("app-skoly")
        .unwrap()
        .push("steward".into());
    store.upsert_group(held).await.unwrap();
    let quiet = apply_realm_files(&store, &files, &opts)
        .await
        .expect("third apply");
    assert!(!quiet.has_changes(), "nothing the file declares is missing");
}

/// A group that lost only a realm role behind an unchanged file is applied again too.
#[tokio::test]
async fn a_group_that_lost_a_realm_role_alone_gets_it_back() {
    let doc = json!({
        "realm": "lost",
        "groups": [{"name": "readers", "path": "/readers", "realmRoles": ["reader"]}],
        "roles": {"realm": [{"name": "reader"}]}
    });
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let files = [("realm.json".to_owned(), doc)];
    let opts = ApplyOptions::default();
    apply_realm_files(&store, &files, &opts)
        .await
        .expect("apply");
    let mut held = store
        .get_group_by_path("lost", None, "/readers")
        .await
        .unwrap()
        .unwrap();
    held.realm_roles.clear();
    store.upsert_group(held).await.unwrap();
    let again = apply_realm_files(&store, &files, &opts)
        .await
        .expect("re-apply");
    assert!(
        again.has_changes(),
        "the lost realm role forces a real apply"
    );
    let held = store
        .get_group_by_path("lost", None, "/readers")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(held.realm_roles, ["reader"]);
}

/// A password the file seeds is set once, when the user has none: `jc` applies its realm files on
/// every start, and a person who changed the first administrator's password keeps the new one
/// across restarts and upgrades (P-23). An OTP seed is kept the same way.
#[tokio::test]
async fn a_restart_keeps_a_password_changed_after_the_user_was_created() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let mut opts = default_opts();
    opts.env_vars
        .insert("FIRST_PW".into(), "FirstPassword123!Valid".into());
    let doc = json!({
        "realm": "restart",
        "users": [{
            "username": "first-admin",
            "enabled": true,
            "credentials": [
                {"type": "password", "value": "$(env:FIRST_PW)"},
                {"type": "otp", "secretData": json!({"value": "$(env:USER_TOTP_DEMO_STEWARD)"}).to_string()}
            ]
        }]
    });
    let files = [("u.json".to_owned(), doc)];
    apply_realm_files(&store, &files, &opts)
        .await
        .expect("first start");
    let user = store
        .get_user_by_username_or_email("restart", None, "first-admin")
        .await
        .unwrap()
        .expect("user created");
    let held = store.list_credentials("restart", &user.id).await.unwrap();
    assert_eq!(held.len(), 2, "the first apply seeds both: {held:?}");

    // The person changes the password (the account console writes a credential of its own).
    for old in held.iter().filter(|c| c.credential_type == "password") {
        store
            .delete_credential("restart", &user.id, &old.id)
            .await
            .unwrap();
    }
    let changed = kaisatsu_core::CredentialRecord {
        id: "console-set".into(),
        realm_id: "restart".into(),
        user_id: user.id.clone(),
        credential_type: "password".into(),
        user_label: None,
        created_date: 1,
        secret_data: "the-hash-the-person-set".into(),
        credential_data: String::new(),
        temporary: false,
    };
    store.upsert_credential(changed).await.unwrap();

    // A restart with the same files takes the unchanged fast path; one after an upgrade (the
    // files differ) applies them again, and neither may touch the password.
    for version in ["same", "upgraded", "upgraded again"] {
        let mut doc = files[0].1.clone();
        if version != "same" {
            doc["displayName"] = json!(version);
        }
        apply_realm_files(&store, &[("u.json".to_owned(), doc)], &opts)
            .await
            .expect("restart");
    }
    let after = store.list_credentials("restart", &user.id).await.unwrap();
    let passwords: Vec<_> = after
        .iter()
        .filter(|c| c.credential_type == "password")
        .map(|c| c.secret_data.as_str())
        .collect();
    assert_eq!(passwords, ["the-hash-the-person-set"], "{after:?}");
    assert_eq!(
        after.iter().filter(|c| c.credential_type == "otp").count(),
        1,
        "{after:?}"
    );
}

/// A stored sign-out or identity-provider address the outbound guard refuses is kept by the
/// import and named in its warnings (an import that failed on it would stop jc's start); a host
/// listed in `outbound_private_hosts` passes without a warning.
#[tokio::test]
async fn a_refused_outbound_address_is_kept_with_a_warning_not_a_failed_import() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let internal = "http://portal.portal.svc.cluster.local:8080/api/v1/auth/backchannel-logout";
    let mut doc = ri_fixture();
    let clients = doc["clients"].as_array_mut().unwrap();
    let edge = clients
        .iter_mut()
        .find(|c| c["clientId"] == "edge")
        .expect("edge client");
    edge["attributes"]["backchannel.logout.url"] = json!(internal);
    let opts = default_opts();
    let report = apply_realm_files(&store, &[("ri_dev.json".into(), doc.clone())], &opts)
        .await
        .expect("the import still applies");
    let warned: Vec<_> = report
        .warnings
        .iter()
        .filter(|w| w.contains("backchannel.logout.url"))
        .collect();
    assert_eq!(warned.len(), 1, "{:?}", report.warnings);
    assert!(warned[0].contains("outbound_private_hosts"), "{warned:?}");
    let kept = store
        .get_client_by_client_id("dev", "edge")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kept.attributes["backchannel.logout.url"], internal);

    let listed = ApplyOptions {
        outbound_private_hosts: vec!["portal.portal.svc.cluster.local:8080".into()],
        ..default_opts()
    };
    let warnings = check_realm_value(&doc, &listed).unwrap();
    assert!(
        warnings.iter().all(|w| w.starts_with("$.defaultGroups")),
        "{warnings:?}"
    );

    // An identity provider's key address the guard refuses is named too.
    doc["identityProviders"][0]["config"]["jwksUrl"] = json!("https://10.0.0.1/jwks");
    let warnings = check_realm_value(&doc, &listed).unwrap();
    assert!(
        warnings.iter().any(|w| w.contains("jwksUrl")),
        "{warnings:?}"
    );
}

#[tokio::test]
async fn file_groups_become_rows_and_the_export_reads_them_back() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let doc = json!({
        "realm": "members",
        "groups": [{ "name": "staff", "path": "/staff", "subGroups": [{ "name": "night", "path": "/staff/night" }] }],
        "users": [{ "username": "ada", "groups": ["/staff/night"] }]
    });
    apply_realm_files(&store, &[("members.json".into(), doc)], &default_opts())
        .await
        .unwrap();
    let ada = store
        .get_user_by_username_or_email("members", None, "ada")
        .await
        .unwrap()
        .unwrap();
    assert!(ada.groups.is_empty(), "the document keeps no path");
    let joined = store.list_user_groups("members", &ada.id).await.unwrap();
    assert_eq!(joined.len(), 1);
    assert_eq!(joined[0].path, "/staff/night");
    let exported = export_realm(&store, "members", true).await.unwrap();
    assert_eq!(exported["users"][0]["groups"], json!(["/staff/night"]));
}
