#![forbid(unsafe_code)]
//! Shared isolation test suite for `kaisatsu-store-memory` and `kaisatsu-store-pg` (W2, W3, W4).
//! Runs against `MemoryStore` always, and against `PgStore` when `KAISATSU_TEST_DATABASE_URL` is set
//! (prints a `SKIP` line when unset, as required by W2).

use kaisatsu_core::*;
use kaisatsu_store_memory::MemoryStore;
use kaisatsu_store_pg::{MIGRATION_0001_SQL, MIGRATION_0002_SQL, MIGRATION_0003_SQL};

mod common;
use odata_table::QueryOptions;
use std::collections::HashMap;
use std::sync::Arc;

async fn run_shared_isolation_suite(store: Arc<dyn Store>) {
    let authorizer = LocalAuthorizer;
    store
        .upsert_realm(RealmConfig {
            id: "dev".into(),
            realm: "dev".into(),
            ..Default::default()
        })
        .await
        .expect("realm");

    // Seed two organizations A (helsinki) and B (zilina) with same-named leaf groups and users
    for (org_id, domain) in [("helsinki", "hel.fi"), ("zilina", "zilina.sk")] {
        store
            .upsert_organization(Organization {
                id: org_id.into(),
                realm_id: "dev".into(),
                name: org_id.into(),
                alias: org_id.into(),
                enabled: true,
                description: String::new(),
                domains: vec![OrganizationDomain {
                    id: format!("dom-{org_id}"),
                    realm_id: "dev".into(),
                    org_id: org_id.into(),
                    name: domain.into(),
                    verified: true,
                    verification_token: None,
                    challenge_hash: None,
                    challenge_expires_at: None,
                }],
                attributes: HashMap::new(),
                managed_by: None,
            })
            .await
            .expect("org");

        store
            .upsert_group(GroupRepresentation {
                id: format!("grp-{org_id}-members"),
                realm_id: "dev".into(),
                org_id: Some(org_id.into()),
                name: "members".into(),
                path: format!("/{org_id}/members"),
                ..Default::default()
            })
            .await
            .expect("group");

        store
            .create_user(UserRepresentation {
                id: format!("user-{org_id}-1"),
                realm_id: "dev".into(),
                org_id: Some(org_id.into()),
                username: format!("alice@{domain}"),
                email: Some(format!("alice@{domain}")),
                enabled: true,
                email_verified: true,
                ..Default::default()
            })
            .await
            .expect("user");

        store
            .upsert_client(ClientRepresentation {
                id: format!("client-uuid-{org_id}"),
                realm_id: "dev".into(),
                org_id: Some(org_id.into()),
                client_id: format!("app-{org_id}"),
                ..Default::default()
            })
            .await
            .expect("client");

        store
            .append_event(AuditEvent {
                id: format!("ev-{org_id}"),
                realm_id: "dev".into(),
                org_id: Some(org_id.into()),
                time: 1000,
                event_type: "LOGIN".into(),
                client_id: Some(format!("app-{org_id}")),
                user_id: Some(format!("user-{org_id}-1")),
                session_id: None,
                ip_address: "127.0.0.1".into(),
                error: None,
                details: Default::default(),
            })
            .await
            .expect("event");
    }

    let admin_a = RequestCtx::new(
        "dev",
        Principal {
            subject: Some("user-helsinki-1".into()),
            client_id: None,
            realm_roles: Vec::new(),
            client_roles: HashMap::new(),
            organizations: vec!["helsinki".into()],
            org_roles: HashMap::from([("helsinki".into(), vec!["org-admin".into()])]),
            second_factor: true,
        },
    )
    .with_org("helsinki");

    let allowed_a = admin_a.allowed_orgs();

    // 1. Organization A admin lists people, groups, clients, events: sees only A, zero of B
    let people_a = store
        .list_users("dev", allowed_a.as_deref(), None, None, None, false, 0, 100)
        .await
        .expect("list users");
    assert_eq!(people_a.len(), 1);
    assert_eq!(people_a[0].id, "user-helsinki-1");

    // 2. Organization A admin fetches B's person, group, client by ID: gets None (404)
    assert!(
        store
            .get_user_by_id("dev", allowed_a.as_deref(), "user-zilina-1")
            .await
            .expect("get user")
            .is_none()
    );
    assert!(
        store
            .get_group("dev", allowed_a.as_deref(), "grp-zilina-members")
            .await
            .expect("get group")
            .is_none()
    );
    assert!(
        store
            .get_client_by_uuid("dev", allowed_a.as_deref(), "client-uuid-zilina")
            .await
            .expect("get client")
            .is_none()
    );

    // 3. OData query on People, Groups, Applications, Events with $count=true counts only A
    for set in ["People", "Groups", "Applications", "Events"] {
        let opts = QueryOptions {
            count: Some(true),
            ..QueryOptions::default()
        };
        let ans = store
            .query_entity_set(&admin_a, &authorizer, set, &opts, 100)
            .await
            .expect("odata query");
        assert_eq!(
            ans.count,
            Some(1),
            "set {set} leaked rows across organizations"
        );
        assert_eq!(ans.rows.len(), 1);
    }

    // 4. Secret fields cannot be filtered or ordered in OData (W4: 400 Bad Request / UnknownProperty)
    let csdl = kaisatsu_odata_service().model();
    let edm = odata_url::EdmModel(&csdl);
    let filter_res = odata_url::parse(
        "People?$filter=passwordHash%20ne%20null",
        &edm,
        &odata_url::Limits::strict(),
    );
    if let Ok(odata_url::ODataUri::Resource { options, .. }) = filter_res {
        let err = store
            .query_entity_set(&admin_a, &authorizer, "People", &options, 100)
            .await
            .expect_err("filtering on secret field must fail");
        assert_eq!(err.status_code(), 400);
    }
    let order_res = odata_url::parse(
        "Credentials?$orderby=secretData",
        &edm,
        &odata_url::Limits::strict(),
    );
    if let Ok(odata_url::ODataUri::Resource { options, .. }) = order_res {
        let err = store
            .query_entity_set(&admin_a, &authorizer, "Credentials", &options, 100)
            .await
            .expect_err("ordering on secret field must fail");
        assert_eq!(err.status_code(), 400);
    }
}

#[tokio::test]
async fn memory_store_passes_isolation_and_odata_secret_field_refusal() {
    let store = Arc::new(MemoryStore::default());
    run_shared_isolation_suite(store).await;
}

#[test]
fn migration_ddl_enforces_rls_and_leakproof_indexes_on_every_table() {
    let tables = [
        "realms",
        "organizations",
        "organization_domains",
        "people",
        "memberships",
        "credentials",
        "groups",
        "group_members",
        "roles",
        "role_mappings",
        "applications",
        "client_scopes",
        "identity_providers",
        "signing_keys",
        "auth_session",
        "sso_session",
        "client_session",
        "offline_session",
        "refresh_token_family",
        "revoked_token",
        "login_failure",
        "consent",
        "auth_code",
        "par_request",
        "required_actions",
        "email_outbox",
        "events",
        "admin_events",
        "branding_blocks",
    ];
    for t in tables {
        assert!(
            MIGRATION_0001_SQL.contains(&format!(
                "ALTER TABLE kaisatsu.{t} ENABLE ROW LEVEL SECURITY;"
            )),
            "table {t} missing ENABLE ROW LEVEL SECURITY"
        );
        assert!(
            MIGRATION_0001_SQL.contains(&format!(
                "ALTER TABLE kaisatsu.{t} FORCE ROW LEVEL SECURITY;"
            )),
            "table {t} missing FORCE ROW LEVEL SECURITY"
        );
    }
    for t in ["webauthn_credential", "idp_links"] {
        assert!(
            MIGRATION_0002_SQL.contains(&format!(
                "ALTER TABLE kaisatsu.{t} ENABLE ROW LEVEL SECURITY;"
            )),
            "0002 table {t} missing ENABLE ROW LEVEL SECURITY"
        );
        assert!(
            MIGRATION_0002_SQL.contains(&format!(
                "ALTER TABLE kaisatsu.{t} FORCE ROW LEVEL SECURITY;"
            )),
            "0002 table {t} missing FORCE ROW LEVEL SECURITY"
        );
    }
    for stmt in ["ENABLE", "FORCE"] {
        assert!(
            MIGRATION_0003_SQL.contains(&format!(
                "ALTER TABLE kaisatsu.device_code {stmt} ROW LEVEL SECURITY;"
            )),
            "0003 table device_code missing {stmt} ROW LEVEL SECURITY"
        );
    }
    // Verify required W2 leakproof indexes
    assert!(MIGRATION_0001_SQL.contains("(realm_id, token_hash)"));
    assert!(MIGRATION_0001_SQL.contains("(realm_id, username_lower)"));
    assert!(MIGRATION_0001_SQL.contains("(realm_id, email_lower)"));
}

#[tokio::test]
async fn pg_store_passes_isolation_and_odata_secret_field_refusal() {
    let Some(db) =
        common::test_db("pg_store_passes_isolation_and_odata_secret_field_refusal").await
    else {
        return;
    };
    let store = Arc::new(db.store(kaisatsu_crypto::Kek::generate_for_tests()).await);
    run_shared_isolation_suite(store).await;
    db.drop_db().await;
}
