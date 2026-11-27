#![forbid(unsafe_code)]
//! The boot check (C1, A6) on the shared test database: alone in its binary, because it adds a
//! table without forced RLS for a moment, which any other boot check running beside it would see.

mod common;

use common::*;
use dataroom_core::{DataroomError, Store};
use dataroom_store_pg::{set_tenant_config, PgStore};

/// A6: the boot check refuses a superuser pool and a table without forced RLS, and passes the
/// request role on the migrated schema; that role sees no row of another organization.
#[tokio::test]
async fn a6_postgres_rls_tenant_isolation_and_boot_check() {
    use dataroom_core::{
        Catalogue, CatalogueId, OrgId, Publisher, RecordState, RequestCtx, Subject, ThemeTokens,
        Visibility,
    };
    use jc_time::TimestampTz;
    use std::collections::BTreeMap;
    use uuid::Uuid;

    let Some(pg) = pg("a6_postgres_rls_tenant_isolation_and_boot_check").await else {
        return;
    };
    // The migration owner (a superuser) and the request role without SUPERUSER or BYPASSRLS
    // (a superuser pool is refused for being one, which proves nothing about RLS).
    let (admin_pool, request_pool) = (pg.admin.clone(), app_pool(&pg.url, 1).await);

    // The superuser pool is refused; the request role passes on the migrated schema.
    assert!(PgStore::connect_and_verify(admin_pool.clone())
        .await
        .is_err());
    PgStore::connect_and_verify(request_pool.clone())
        .await
        .expect("request role passes the boot check on the migrated schema");

    // Boot check refusal for a newly created table without forced RLS (C1, A6).
    sqlx::query("CREATE TABLE dataroom.zz_a6_no_forced_rls (org_id uuid NOT NULL)")
        .execute(&admin_pool)
        .await
        .expect("create table without forced RLS");
    let refused = PgStore::connect_and_verify(request_pool.clone()).await;
    sqlx::query("DROP TABLE dataroom.zz_a6_no_forced_rls")
        .execute(&admin_pool)
        .await
        .expect("drop test table");
    let refused = refused.err();
    assert!(
        matches!(&refused, Some(DataroomError::Storage(msg)) if msg.contains("zz_a6_no_forced_rls")),
        "the boot check refuses and names the table: {refused:?}"
    );

    let store = PgStore::from_pool_unchecked(admin_pool.clone());
    let org_a = OrgId::from_uuid(Uuid::now_v7());
    let cat_a = CatalogueId::from_uuid(Uuid::now_v7());
    let org_b = OrgId::from_uuid(Uuid::now_v7());
    let cat_b = CatalogueId::from_uuid(Uuid::now_v7());

    for (org, cat, host) in [
        (
            org_a,
            cat_a,
            format!("a-{}.example.org", Uuid::now_v7().simple()),
        ),
        (
            org_b,
            cat_b,
            format!("b-{}.example.org", Uuid::now_v7().simple()),
        ),
    ] {
        store
            .upsert_catalogue(Catalogue {
                org_id: org,
                branding: None,
                hidden_datasets: Default::default(),
                id: cat,
                name: "shared".into(),
                host,
                title: BTreeMap::from([("sk".into(), "Cat".into())]),
                description: BTreeMap::new(),
                homepage: None,
                default_language: "sk".into(),
                languages: vec!["sk".into()],
                main_profile: "dcat-ap-3.0.1".into(),
                active_profiles: Vec::new(),
                default_licence: Some("cc-by".into()),
                publisher_uri: None,
                publisher_name: None,
                contact_point: None,
                theme: ThemeTokens::default(),
                allow_cross_org_harvest: true,
                allowed_outbound_hosts: Vec::new(),
                applicable_legislation: Vec::new(),
                modified: TimestampTz::now(),
                deleted: None,
                public_base: None,
                portal_base: None,
                left_hosts: Vec::new(),
            })
            .await
            .expect("upsert catalogue");
    }

    let ctx_a = RequestCtx::new(
        org_a,
        cat_a,
        Subject::authenticated("alice", vec![org_a], Vec::new()),
    );
    let ctx_b = RequestCtx::new(
        org_b,
        cat_b,
        Subject::authenticated("bob", vec![org_b], Vec::new()),
    );

    store
        .upsert_publisher(
            &ctx_b,
            Publisher {
                org_id: org_b,
                badge_initials: None,
                badge_colour: None,
                declared_by_instance: None,
                catalogue_id: cat_b,
                id: "pub-b".into(),
                name: "city-b".into(),
                title: BTreeMap::from([("sk".into(), "City B".into())]),
                description: BTreeMap::new(),
                uri: "https://b.example.org/org".into(),
                publisher_type: None,
                identifier: None,
                email: None,
                homepage: None,
                logo_url: None,
                contact_point: None,
                state: RecordState::Active,
                merged_into: None,
                editor_grants: Vec::new(),
                created: TimestampTz::now(),
                modified: TimestampTz::now(),
            },
        )
        .await
        .expect("upsert pub b");

    // Verify as non-BYPASSRLS role `dataroom_app_test` that org A cannot see org B rows
    let mut tx = admin_pool.begin().await.expect("begin tx");
    sqlx::query("SET LOCAL ROLE dataroom_app_test")
        .execute(&mut *tx)
        .await
        .expect("set role dataroom_app_test");
    set_tenant_config(&mut tx, org_a, Some(cat_a))
        .await
        .expect("set tenant config A");

    let count_b_from_a: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM dataroom.cat_publisher")
        .fetch_one(&mut *tx)
        .await
        .expect("count publishers as A");
    assert_eq!(count_b_from_a.0, 0, "RLS must hide org B rows from org A");
    tx.rollback().await.ok();

    assert!(store
        .get_publisher(&ctx_a, "city-b")
        .await
        .unwrap()
        .is_none());
    let _ = Visibility::Internal;
}
