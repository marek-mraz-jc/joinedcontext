#![forbid(unsafe_code)]
//! What an apply writes and what its plan reports (S3-38, S3-39, S3-40, S3-28):
//! - every section shows a changed field in the plan, and an unchanged file shows none;
//! - a section kaisatsu cannot store is refused with its JSON path;
//! - a person's run-time state (required actions, attributes, administrator grants) survives a
//!   changed file, while a role only the file gave ends when the file drops it;
//! - a realm is keyed by its name, a stored realm keeps its id;
//! - the shipped `jc-portal.json` plans as the golden `fixtures/jc-portal.plan.json`.

use kaisatsu_core::{RealmConfig, RoleMappingRecord, Store, UserRepresentation};
use kaisatsu_import::*;
use kaisatsu_store_memory::MemoryStore;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

fn opts() -> ApplyOptions {
    ApplyOptions {
        mode: ApplyMode::NoDelete,
        dry_run: false,
        allow_literal_secrets: false,
        secrets_dir: None,
        env_vars: BTreeMap::from([
            ("APP_SECRET".into(), "app-secret-123".into()),
            ("CLIENT_SECRET_PORTAL_API".into(), "portal-api-123".into()),
            (
                "CLIENT_SECRET_PORTAL_RECONCILER".into(),
                "portal-reconciler-123".into(),
            ),
        ]),
        outbound_private_hosts: Vec::new(),
    }
}

fn city() -> Value {
    json!({
        "realm": "city",
        "displayName": "City",
        "clientScopes": [{"name": "city-scope", "protocol": "openid-connect", "description": "one"}],
        "roles": {
            "realm": [{"name": "reader", "description": "reads"}],
            "client": {"city-app": [{"name": "editor", "description": "edits"}]}
        },
        "groups": [{"name": "staff", "path": "/staff", "attributes": {"unit": ["a"]}}],
        "clients": [{
            "clientId": "city-app",
            "secret": "$(env:APP_SECRET)",
            "redirectUris": ["https://app.example/cb"],
            "directAccessGrantsEnabled": false,
            "protocolMappers": [{
                "name": "unit",
                "protocol": "openid-connect",
                "protocolMapper": "oidc-usermodel-attribute-mapper",
                "config": {"user.attribute": "unit", "claim.name": "unit"}
            }]
        }],
        "organizations": [{"name": "Helsinki", "alias": "hel", "domains": [{"name": "hel.example", "verified": true}]}],
        "requiredActions": [{"alias": "CONFIGURE_TOTP", "name": "Configure OTP", "providerId": "CONFIGURE_TOTP", "enabled": true, "priority": 10}],
        "users": [{"username": "ana", "firstName": "Ana", "realmRoles": ["reader"], "groups": ["/staff"]}]
    })
}

fn files(doc: &Value) -> [(String, Value); 1] {
    [("city.json".to_owned(), doc.clone())]
}

async fn plan(store: &Arc<dyn Store>, doc: &Value, mode: ApplyMode) -> ApplyReport {
    let mut o = opts();
    o.dry_run = true;
    o.mode = mode;
    apply_realm_files(store, &files(doc), &o)
        .await
        .expect("plan")
}

fn line<'a>(report: &'a ApplyReport, kind: &str, id: &str) -> &'a ObjectChange {
    report
        .changes
        .iter()
        .find(|c| c.kind == kind && c.id == id)
        .unwrap_or_else(|| panic!("no {kind} {id} in {:?}", report.changes))
}

#[tokio::test]
async fn a_changed_field_shows_in_the_plan_for_every_section() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    apply_realm_files(&store, &files(&city()), &opts())
        .await
        .expect("first apply");

    // The same file, read again past the checksum fast path: nothing differs.
    let same = plan(&store, &city(), ApplyMode::Full).await;
    let moved: Vec<_> = same
        .changes
        .iter()
        .filter(|c| c.action != ChangeAction::Unchanged)
        .collect();
    assert!(moved.is_empty(), "an unchanged file plans no change: {moved:?}");

    let mut doc = city();
    doc["revokeRefreshToken"] = json!(false);
    doc["clientScopes"][0]["attributes"] = json!({"include.in.token.scope": "false"});
    doc["roles"]["realm"][0]["description"] = json!("reads more");
    doc["roles"]["client"]["city-app"][0]["description"] = json!("edits more");
    doc["groups"][0]["attributes"] = json!({"unit": ["b"]});
    doc["clients"][0]["directAccessGrantsEnabled"] = json!(true);
    doc["organizations"][0]["description"] = json!("capital");
    doc["requiredActions"][0]["priority"] = json!(20);
    doc["users"][0]["lastName"] = json!("Novak");
    let report = plan(&store, &doc, ApplyMode::NoDelete).await;
    for (kind, id, field) in [
        ("Realm", "city", "revokeRefreshToken"),
        ("ClientScope", "city-scope", "attributes"),
        ("Role", "reader", "description"),
        ("ClientRole", "city-app/editor", "description"),
        ("Group", "/staff", "attributes"),
        ("Client", "city-app", "directAccessGrantsEnabled"),
        ("Organization", "hel", "description"),
        ("RequiredAction", "CONFIGURE_TOTP", "priority"),
        ("User", "ana", "lastName"),
    ] {
        let c = line(&report, kind, id);
        assert_eq!(c.action, ChangeAction::Update, "{kind} {id}: {c:?}");
        assert_eq!(c.changed_fields, [field], "{kind} {id}");
    }

    // A changed mapper is a change of the client; a changed secret is named, never shown.
    let mut doc = city();
    doc["clients"][0]["protocolMappers"][0]["config"]["claim.name"] = json!("org_unit");
    let mut o = opts();
    o.dry_run = true;
    o.env_vars.insert("APP_SECRET".into(), "rotated-456".into());
    let report = apply_realm_files(&store, &files(&doc), &o)
        .await
        .expect("plan");
    let c = line(&report, "Client", "city-app");
    assert_eq!(c.changed_fields, ["protocolMappers", "secret"]);
    let printed = serde_json::to_string(&report).unwrap() + &report.to_markdown();
    assert!(!printed.contains("rotated-456") && !printed.contains("app-secret-123"));
    assert_eq!(report.exit_code(), EXIT_CHANGES_OR_APPLIED);
}

#[tokio::test]
async fn organizations_client_roles_and_required_actions_are_written() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    apply_realm_files(&store, &files(&city()), &opts())
        .await
        .expect("apply");
    let org = store
        .get_organization("city", "hel")
        .await
        .unwrap()
        .expect("organization stored");
    assert_eq!(org.name, "Helsinki");
    assert_eq!(org.domains.len(), 1);
    assert!(
        !org.domains[0].verified,
        "a file's domain waits for the DNS proof"
    );
    let app = store
        .get_client_by_client_id("city", "city-app")
        .await
        .unwrap()
        .expect("client");
    let roles = store.list_roles("city", Some(&app.id)).await.unwrap();
    assert_eq!(
        roles.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        ["editor"]
    );
    assert!(roles[0].client_role);
    let actions = store.list_required_actions("city").await.unwrap();
    assert!(actions.iter().any(|a| a.alias == "CONFIGURE_TOTP"));

    // A verified domain keeps its proof across a re-apply.
    let mut verified = org.clone();
    verified.domains[0].verified = true;
    store.upsert_organization(verified).await.unwrap();
    let mut doc = city();
    doc["displayName"] = json!("City 2");
    apply_realm_files(&store, &files(&doc), &opts())
        .await
        .expect("re-apply");
    let org = store.get_organization("city", "hel").await.unwrap().unwrap();
    assert!(org.domains[0].verified);

    // --prune drops a client role the file no longer names.
    doc["roles"]["client"]["city-app"] = json!([]);
    let mut o = opts();
    o.mode = ApplyMode::Full;
    let report = apply_realm_files(&store, &files(&doc), &o)
        .await
        .expect("prune");
    assert_eq!(
        line(&report, "ClientRole", "city-app/editor").action,
        ChangeAction::Delete
    );
    assert!(
        store
            .list_roles("city", Some(&app.id))
            .await
            .unwrap()
            .is_empty()
    );

    // A client role for a client neither stored nor in the file is refused by its path.
    let mut doc = city();
    doc["roles"]["client"] = json!({"ghost": [{"name": "x"}]});
    let err = apply_realm_files(&store, &files(&doc), &opts())
        .await
        .expect_err("unknown client");
    assert!(err.to_string().contains("$.roles.client.ghost"), "{err}");
}

#[tokio::test]
async fn an_unsupported_section_is_refused_with_its_name() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let mut doc = city();
    doc["components"] = json!({
        "org.keycloak.storage.UserStorageProvider": [{"name": "ldap", "providerId": "ldap"}]
    });
    let err = apply_realm_files(&store, &files(&doc), &opts())
        .await
        .expect_err("user federation is refused");
    assert!(
        err.to_string()
            .contains("$.components[\"org.keycloak.storage.UserStorageProvider\"]"),
        "{err}"
    );
    assert!(store.get_realm("city").await.unwrap().is_none(), "nothing written");

    // An export's signing keys and user profile are named, their configuration never printed.
    doc["components"] = json!({
        "org.keycloak.keys.KeyProvider": [{"name": "rsa", "config": {"privateKey": ["MIIE-secret"]}}],
        "org.keycloak.userprofile.UserProfileProvider": []
    });
    doc["defaultGroups"] = json!(["/staff"]);
    doc["browserSecurityHeaders"] = json!({"xFrameOptions": "DENY"});
    doc["organizations"][0]["members"] = json!([{"username": "ana"}]);
    let report = apply_realm_files(&store, &files(&doc), &opts())
        .await
        .expect("apply with warnings");
    let joined = report.warnings.join("\n");
    for named in [
        "$.components[\"org.keycloak.keys.KeyProvider\"]",
        "$.defaultGroups",
        "$.browserSecurityHeaders",
        "$.organizations[hel].members",
    ] {
        assert!(joined.contains(named), "{named} missing in {joined}");
    }
    assert!(!joined.contains("MIIE-secret"));
    assert!(!joined.contains("UserProfileProvider"), "an empty section is no warning");
}

#[tokio::test]
async fn a_persons_runtime_changes_survive_an_apply() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let mut doc = city();
    doc["roles"]["realm"] = json!([{"name": "reader"}, {"name": "auditor"}, {"name": "portal-admin"}]);
    doc["users"][0]["realmRoles"] = json!(["reader", "auditor"]);
    doc["users"][0]["attributes"] = json!({"locale": ["fi"]});
    apply_realm_files(&store, &files(&doc), &opts())
        .await
        .expect("first apply");
    let ana = store
        .get_user_by_username_or_email("city", None, "ana")
        .await
        .unwrap()
        .expect("ana");

    // At run time: a required action, an IdP link attribute, a disabled flag and a role the
    // Portal mapped through the admin API.
    let mut live = ana.clone();
    live.required_actions = vec!["UPDATE_PASSWORD".into()];
    live.attributes
        .insert("idp_link".into(), vec!["keycloak-old:42".into()]);
    live.enabled = false;
    store.update_user(live).await.unwrap();
    store
        .add_role_mapping(RoleMappingRecord {
            id: "rm-1".into(),
            realm_id: "city".into(),
            org_id: None,
            principal_kind: "user".into(),
            principal_id: ana.id.clone(),
            role_id: "portal-admin".into(),
            role_name: "portal-admin".into(),
            client_uuid: None,
        })
        .await
        .unwrap();
    let mut held = ana.realm_roles.clone();
    held.push("portal-admin".into());
    store
        .set_user_grants("city", &ana.id, &held, &ana.client_roles)
        .await
        .unwrap();

    // The file changes the name and the locale and drops `auditor`.
    doc["users"][0]["firstName"] = json!("Anna");
    doc["users"][0]["attributes"] = json!({"locale": ["sv"]});
    doc["users"][0]["realmRoles"] = json!(["reader"]);
    let report = apply_realm_files(&store, &files(&doc), &opts())
        .await
        .expect("second apply");
    let c = line(&report, "User", "ana");
    assert_eq!(c.action, ChangeAction::Update);
    assert_eq!(c.changed_fields, ["attributes", "firstName", "realmRoles"]);

    let after: UserRepresentation = store
        .get_user_by_id("city", None, &ana.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.first_name.as_deref(), Some("Anna"));
    assert_eq!(after.required_actions, ["UPDATE_PASSWORD"]);
    assert!(!after.enabled, "the file names no enabled flag");
    assert_eq!(after.attributes["locale"], ["sv"]);
    assert_eq!(after.attributes["idp_link"], ["keycloak-old:42"]);
    let mut roles = after.realm_roles.clone();
    roles.sort();
    assert_eq!(
        roles,
        ["portal-admin", "reader"],
        "the mapped role stays, the role only the file gave ends"
    );
}

#[tokio::test]
async fn a_realm_is_keyed_by_its_name() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let mut doc = city();
    doc["id"] = json!("0b1c2d3e-uuid-of-an-export");
    let report = apply_realm_files(&store, &files(&doc), &opts())
        .await
        .expect("apply");
    assert!(
        report.warnings.iter().any(|w| w.contains("$.id")),
        "{:?}",
        report.warnings
    );
    let realm = store.get_realm("city").await.unwrap().expect("realm");
    assert_eq!(realm.id, "city");
    assert!(
        store
            .get_user_by_username_or_email("city", None, "ana")
            .await
            .unwrap()
            .is_some(),
        "the people are stored under the name the admin routes use"
    );

    // A realm stored earlier under another id keeps it: its rows stay reachable.
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    store
        .upsert_realm(RealmConfig {
            id: "legacy-id".into(),
            realm: "city".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    apply_realm_files(&store, &files(&city()), &opts())
        .await
        .expect("apply onto the legacy realm");
    assert_eq!(store.get_realm("city").await.unwrap().unwrap().id, "legacy-id");
    assert!(
        store
            .get_user_by_username_or_email("legacy-id", None, "ana")
            .await
            .unwrap()
            .is_some()
    );
}

/// The shipped realm file plans as recorded: a change to what jc's realm import writes shows here
/// first. On a deliberate change, write the printed plan into the golden file.
#[tokio::test]
async fn the_shipped_jc_realm_plans_as_the_golden() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    let shipped: Value =
        serde_json::from_str(include_str!("../realms/jc-portal.json")).expect("jc-portal.json");
    let report = plan(&store, &shipped, ApplyMode::NoDelete).await;
    let got = serde_json::to_string_pretty(&json!({
        "changes": report.changes,
        "warnings": report.warnings,
    }))
    .unwrap()
        + "\n";
    assert_eq!(got, include_str!("fixtures/jc-portal.plan.json"), "plan:\n{got}");

    // Applied, then planned again past the fast path, the shipped file changes nothing.
    apply_realm_files(&store, &files(&shipped), &opts())
        .await
        .expect("apply");
    let again = plan(&store, &shipped, ApplyMode::Full).await;
    let moved: Vec<_> = again
        .changes
        .iter()
        .filter(|c| c.action != ChangeAction::Unchanged)
        .collect();
    assert!(moved.is_empty(), "{moved:?}");
}
