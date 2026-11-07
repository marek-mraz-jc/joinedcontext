// SPDX-License-Identifier: EUPL-1.2
//! Every backend in this crate is held to the driver contract
//! (`antares_store::contract`), the rules `antares-api` writes against and
//! no backend decides for itself. The Postgres arms run the same two
//! functions in `pg.rs`, where a live server is available.

use antares_model::TenantId;
use antares_sql::store::any::AnyStore;
use antares_sql::store::Store;

fn tenants() -> (TenantId, TenantId) {
    (
        TenantId::new("contracta").expect("tenant"),
        TenantId::new("contractb").expect("tenant"),
    )
}

#[tokio::test]
async fn the_memory_store_keeps_the_driver_contract() {
    let (a, b) = tenants();
    let s = AnyStore::Mem(Store::default());
    antares_store::contract::run_current_state_contract(&s, &a, &b, "memory").await;
    antares_store::contract::run_temporal_contract(&s, &a, &b, "memory").await;
}

#[tokio::test]
async fn the_file_store_keeps_the_driver_contract() {
    let (a, b) = tenants();
    let dir = std::env::temp_dir().join(format!("antares-contract-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tempdir");
    let s = AnyStore::Mem(Store::open_file(&dir).expect("open"));
    antares_store::contract::run_current_state_contract(&s, &a, &b, "file").await;
    antares_store::contract::run_temporal_contract(&s, &a, &b, "file").await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// T-3914: nothing done as one Tenant reaches another's copy of the same
/// ids, on the pairs a careless predicate confuses: two names a LIKE `_`
/// would join, a name that extends the other, and the default Tenant beside
/// a named one.
fn isolation_pairs() -> Vec<(TenantId, TenantId)> {
    let t = |s: &str| TenantId::new(s).expect("tenant");
    vec![
        (t("iso_b"), t("isoxb")),
        (t("isoxb"), t("iso_b")),
        (t("iso"), t("isox")),
        (t("isonamed"), TenantId::default()),
    ]
}

#[tokio::test]
async fn the_memory_store_keeps_tenants_apart() {
    let s = AnyStore::Mem(Store::default());
    for (n, (a, b)) in isolation_pairs().iter().enumerate() {
        antares_store::contract::run_tenant_isolation_contract(&s, &s, a, b, &format!("mem{n}"))
            .await;
    }
}

#[tokio::test]
async fn the_file_store_keeps_tenants_apart() {
    let dir = std::env::temp_dir().join(format!("antares-isolation-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tempdir");
    let s = AnyStore::Mem(Store::open_file(&dir).expect("open"));
    for (n, (a, b)) in isolation_pairs().iter().enumerate() {
        antares_store::contract::run_tenant_isolation_contract(&s, &s, a, b, &format!("file{n}"))
            .await;
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The wire grammar refuses the names a quoted or LIKE predicate would
/// misread, so no store ever sees them (5.5.3).
#[test]
fn a_tenant_name_with_sql_metacharacters_never_reaches_a_store() {
    for raw in ["a'b", "a%", "a;b", "a b", "a\\b", "a\"b", ""] {
        assert!(TenantId::new(raw).is_err(), "{raw:?} must be refused");
    }
    assert!(
        TenantId::new("a_b").is_ok(),
        "`_` is legal, so the stores are tested with it"
    );
}
