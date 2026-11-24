#![forbid(unsafe_code)]
//! Shared isolation test suite for `kaisatsu-store-memory` and `kaisatsu-store-pg` (W2, W3, W4).
//! Runs against `MemoryStore` always, and against `PgStore` when `KAISATSU_TEST_DATABASE_URL` is set
//! (prints a `SKIP` line when unset, as required by W2).

use kaisatsu_store_pg::{
    MIGRATION_0001_SQL, MIGRATION_0002_SQL, MIGRATION_0003_SQL, MIGRATION_0004_SQL,
};
use std::sync::Arc;

mod common;
#[path = "../../store-memory/tests/isolation_cases/mod.rs"]
mod isolation_cases;
use isolation_cases::*;

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
        assert!(
            MIGRATION_0004_SQL.contains(&format!(
                "ALTER TABLE kaisatsu.grant_cleanup_0004 {stmt} ROW LEVEL SECURITY;"
            )),
            "0004 table grant_cleanup_0004 missing {stmt} ROW LEVEL SECURITY"
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
