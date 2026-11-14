#![forbid(unsafe_code)]
//! Tenant isolation proved in the database itself, through the request role: forced RLS binds
//! the organization and catalogue the store sets for each transaction, fails closed when none
//! is set, refuses writes into another tenant, and lets host routing read only the catalogue on
//! the host asked (C1, C5, C6, contract "Security").

mod common;

use common::*;
use dataroom_core::{DataroomError, Store};
use dataroom_store_pg::{set_tenant_config, verify_rls_on_pool, PgStore, REQUIRED_RLS_TABLES};
use sqlx::PgPool;

/// Two tenants, each with one row in every tenant table, written through the store.
async fn seed_two_tenants(
    store: &PgStore,
) -> [(dataroom_core::OrgId, dataroom_core::CatalogueId, String); 2] {
    let mut out = Vec::new();
    for n in ["ta", "tb"] {
        let (org, cat) = tenant();
        let host = unique_host(n);
        let ctx = member_ctx(org, cat);
        store
            .upsert_catalogue(catalogue(org, cat, n, &host))
            .await
            .unwrap();
        store
            .upsert_publisher(&ctx, publisher("city"))
            .await
            .unwrap();
        store
            .upsert_dataset(&ctx, dataset("air", "city", "internal"))
            .await
            .unwrap();
        store
            .upsert_resource_view(&ctx, view("v1", "res-air-0", 1))
            .await
            .unwrap();
        store
            .upsert_harvest_source(&ctx, harvest_source("nkod"))
            .await
            .unwrap();
        store
            .upsert_harvest_run(
                &ctx,
                harvest_run("run-1", "hs-nkod", "2026-11-01T10:00:00Z"),
            )
            .await
            .unwrap();
        store
            .record_activity(&ctx, activity("act-1", "pkg-air", "2026-11-01T10:00:00Z"))
            .await
            .unwrap();
        store
            .upsert_shacl_report(&ctx, shacl_report("pkg-air", "eu", true))
            .await
            .unwrap();
        store.upsert_profile(&ctx, profile("p", "1")).await.unwrap();
        store.upsert_page(&ctx, page("about", "sk")).await.unwrap();
        store.upsert_plugin(&ctx, plugin("maps")).await.unwrap();
        out.push((org, cat, host));
    }
    [out.remove(0), out.remove(0)]
}

/// Counts every row of `table` the connection may see, without any WHERE clause of ours.
async fn visible(conn: &mut sqlx::PgConnection, table: &str) -> i64 {
    let (n,): (i64,) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) FROM dataroom.{table}"
    )))
    .fetch_one(&mut *conn)
    .await
    .unwrap_or_else(|e| panic!("count {table}: {e}"));
    n
}

async fn org_rows(conn: &mut sqlx::PgConnection, table: &str, org: dataroom_core::OrgId) -> i64 {
    let (n,): (i64,) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) FROM dataroom.{table} WHERE org_id = $1"
    )))
    .bind(org.as_uuid())
    .fetch_one(&mut *conn)
    .await
    .unwrap_or_else(|e| panic!("count {table}: {e}"));
    n
}

#[tokio::test]
async fn rls_binds_the_tenant_of_the_transaction_in_every_table() {
    let Some(pg) = pg("rls_binds_the_tenant_of_the_transaction_in_every_table").await else {
        return;
    };
    let store = pg.app_store().await;
    let [(org_a, cat_a, _), (org_b, _, _)] = seed_two_tenants(&store).await;

    // Any statement of the request role, even one naming B's organization, sees only A.
    let mut tx = pg.app.begin().await.unwrap();
    set_tenant_config(&mut tx, org_a, Some(cat_a))
        .await
        .unwrap();
    for table in REQUIRED_RLS_TABLES {
        let mine = visible(&mut tx, table).await;
        assert_eq!(mine, org_rows(&mut tx, table, org_a).await, "{table}");
        assert!(mine >= 1, "{table}: A sees its own row");
        assert_eq!(
            org_rows(&mut tx, table, org_b).await,
            0,
            "{table}: B is hidden from A"
        );
    }
    tx.rollback().await.unwrap();

    // The superuser sees both, which is what the request role is kept from.
    let mut admin = pg.admin.acquire().await.unwrap();
    assert_eq!(org_rows(&mut admin, "cat_dataset", org_b).await, 1);
}

#[tokio::test]
async fn rls_fails_closed_without_a_tenant_even_on_a_reused_connection() {
    let Some(pg) = pg("rls_fails_closed_without_a_tenant_even_on_a_reused_connection").await else {
        return;
    };
    let store = pg.app_store().await;
    let [(org_a, cat_a, _), _] = seed_two_tenants(&store).await;

    // One connection: first a tenant transaction (jc.org now reads '' afterwards, not NULL),
    // then statements with no tenant at all. Every table hides every row and none errors.
    let single = app_pool(&pg.url, 1).await;
    let mut conn = single.acquire().await.unwrap();
    {
        let mut tx = sqlx::Connection::begin(&mut *conn).await.unwrap();
        set_tenant_config(&mut tx, org_a, Some(cat_a))
            .await
            .unwrap();
        assert_eq!(visible(&mut tx, "cat_dataset").await, 1);
        tx.commit().await.unwrap();
    }
    for table in REQUIRED_RLS_TABLES {
        assert_eq!(
            visible(&mut conn, table).await,
            0,
            "{table} without a tenant"
        );
    }

    // An organization without a catalogue sees its catalogues but no catalogue's records.
    let mut tx = sqlx::Connection::begin(&mut *conn).await.unwrap();
    set_tenant_config(&mut tx, org_a, None).await.unwrap();
    assert_eq!(visible(&mut tx, "cat_catalogue").await, 1);
    assert_eq!(visible(&mut tx, "cat_dataset").await, 0);
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn one_pooled_connection_serves_hosts_of_two_tenants_in_turn() {
    let Some(pg) = pg("one_pooled_connection_serves_hosts_of_two_tenants_in_turn").await else {
        return;
    };
    let seed = pg.app_store().await;
    let [(org_a, cat_a, host_a), (org_b, cat_b, host_b)] = seed_two_tenants(&seed).await;

    // The request path: host -> catalogue -> tenant context -> query, all on one connection.
    let store = PgStore::connect_and_verify(app_pool(&pg.url, 1).await)
        .await
        .unwrap();
    for _ in 0..2 {
        for (host, org, cat) in [(&host_a, org_a, cat_a), (&host_b, org_b, cat_b)] {
            let found = store
                .resolve_host(host)
                .await
                .unwrap()
                .expect("host resolves");
            assert_eq!((found.org_id, found.id), (org, cat));
            let ctx = member_ctx(found.org_id, found.id);
            let ds = store.list_datasets(&ctx).await.unwrap();
            assert_eq!(ds.len(), 1);
            assert_eq!(ds[0].org_id, org);
        }
    }
}

#[tokio::test]
async fn host_lookup_reads_one_catalogue_and_writes_nothing() {
    let Some(pg) = pg("host_lookup_reads_one_catalogue_and_writes_nothing").await else {
        return;
    };
    let store = pg.app_store().await;
    let [(org_a, _, host_a), (_, _, host_b)] = seed_two_tenants(&store).await;

    let mut tx = pg.app.begin().await.unwrap();
    sqlx::query("SELECT set_config('jc.host', $1, true)")
        .bind(&host_a)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        visible(&mut tx, "cat_catalogue").await,
        1,
        "only the host's catalogue"
    );
    let (seen,): (uuid::Uuid,) = sqlx::query_as("SELECT org_id FROM dataroom.cat_catalogue")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(seen, org_a.as_uuid());
    for table in REQUIRED_RLS_TABLES
        .iter()
        .filter(|t| **t != "cat_catalogue")
    {
        assert_eq!(
            visible(&mut tx, table).await,
            0,
            "{table} through the host lookup"
        );
    }
    // The lookup policy is read-only: an update through it touches nothing.
    let updated = sqlx::query("UPDATE dataroom.cat_catalogue SET title = 'taken over'")
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(updated.rows_affected(), 0);
    tx.rollback().await.unwrap();

    // The store answers each host with its own catalogue.
    assert_ne!(
        store.resolve_host(&host_a).await.unwrap().unwrap().id,
        store.resolve_host(&host_b).await.unwrap().unwrap().id
    );
    assert!(store.resolve_host("").await.unwrap().is_none());
    assert!(store.resolve_host(":8080").await.unwrap().is_none());
}

#[tokio::test]
async fn rls_refuses_writes_into_another_tenant() {
    let Some(pg) = pg("rls_refuses_writes_into_another_tenant").await else {
        return;
    };
    let store = pg.app_store().await;
    let [(org_a, cat_a, _), (org_b, cat_b, _)] = seed_two_tenants(&store).await;

    // Insert a row naming B while acting as A.
    let mut tx = pg.app.begin().await.unwrap();
    set_tenant_config(&mut tx, org_a, Some(cat_a))
        .await
        .unwrap();
    let err = sqlx::query(
        "INSERT INTO dataroom.cat_page (org_id, catalogue_id, slug, lang, doc)
         VALUES ($1, $2, 'evil', 'sk', '{}'::jsonb)",
    )
    .bind(org_b.as_uuid())
    .bind(cat_b.as_uuid())
    .execute(&mut *tx)
    .await
    .expect_err("WITH CHECK refuses another tenant's row");
    assert_eq!(
        err.as_database_error().and_then(|d| d.code()).as_deref(),
        Some("42501"),
        "{err}"
    );
    tx.rollback().await.unwrap();

    // Move A's own row into B.
    let mut tx = pg.app.begin().await.unwrap();
    set_tenant_config(&mut tx, org_a, Some(cat_a))
        .await
        .unwrap();
    let err = sqlx::query("UPDATE dataroom.cat_dataset SET org_id = $1, catalogue_id = $2")
        .bind(org_b.as_uuid())
        .bind(cat_b.as_uuid())
        .execute(&mut *tx)
        .await
        .expect_err("WITH CHECK refuses moving a row out of the tenant");
    assert_eq!(
        err.as_database_error().and_then(|d| d.code()).as_deref(),
        Some("42501")
    );
    tx.rollback().await.unwrap();

    // Delete everything as A: B's rows survive.
    let mut tx = pg.app.begin().await.unwrap();
    set_tenant_config(&mut tx, org_a, Some(cat_a))
        .await
        .unwrap();
    for table in REQUIRED_RLS_TABLES
        .iter()
        .filter(|t| **t != "cat_catalogue")
    {
        sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM dataroom.{table}")))
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    let ctx_b = member_ctx(org_b, cat_b);
    assert!(store.get_dataset(&ctx_b, "air").await.unwrap().is_some());
    assert_eq!(store.list_pages(&ctx_b).await.unwrap().len(), 1);
    assert!(store
        .get_dataset(&member_ctx(org_a, cat_a), "air")
        .await
        .unwrap()
        .is_none());
}

/// Two JC Organizations' catalogues as they sat before ODP-12, both under the installation's
/// one `OrgId`, moved to each Organization's own `OrgId` by the owner.
async fn split_two_organizations(
    pg: &Pg,
    store: &PgStore,
) -> [(dataroom_core::OrgId, dataroom_core::CatalogueId); 2] {
    let (installation, _) = tenant();
    let mut out = Vec::new();
    for name in ["city-a", "city-b"] {
        let (_, cat) = tenant();
        let ctx = member_ctx(installation, cat);
        store
            .upsert_catalogue(catalogue(installation, cat, name, &unique_host(name)))
            .await
            .unwrap();
        store
            .upsert_publisher(&ctx, publisher("city"))
            .await
            .unwrap();
        store
            .upsert_dataset(&ctx, dataset("air", "city", "public"))
            .await
            .unwrap();
        store
            .upsert_harvest_source(&ctx, harvest_source("nkod"))
            .await
            .unwrap();
        let org = dataroom_core::OrgId::of_organization(installation, name);
        let owner = PgStore::from_pool_unchecked(pg.admin.clone());
        assert!(owner.move_catalogue(installation, org, cat).await.unwrap() >= 4);
        assert_eq!(
            owner.move_catalogue(installation, org, cat).await.unwrap(),
            0,
            "a second start moves nothing"
        );
        out.push((org, cat));
    }
    [out.remove(0), out.remove(0)]
}

#[tokio::test]
async fn a_catalogue_of_one_organization_is_invisible_to_another() {
    let Some(pg) = pg("a_catalogue_of_one_organization_is_invisible_to_another").await else {
        return;
    };
    let store = pg.app_store().await;
    let [(org_a, cat_a), (org_b, cat_b)] = split_two_organizations(&pg, &store).await;
    assert_ne!(org_a, org_b);

    // Each Organization reads its own catalogue whole after the move: nothing was lost.
    for (org, cat) in [(org_a, cat_a), (org_b, cat_b)] {
        let ctx = member_ctx(org, cat);
        let air = store.get_dataset(&ctx, "air").await.unwrap().expect("kept");
        assert_eq!(
            air.org_id, org,
            "the stored record names its new tenant too"
        );
        assert_eq!(store.list_harvest_sources(&ctx).await.unwrap().len(), 1);
        let listed = store.list_catalogues(org).await.unwrap();
        assert_eq!(
            listed.iter().map(|c| (c.org_id, c.id)).collect::<Vec<_>>(),
            [(org, cat)]
        );
    }

    // Organization A, naming B's catalogue, sees none of B's rows in any table (the request
    // role is NOSUPERUSER NOBYPASSRLS, so this is the database's row level security).
    let mut conn = pg.app.acquire().await.unwrap();
    let mut tx = sqlx::Connection::begin(&mut *conn).await.unwrap();
    set_tenant_config(&mut tx, org_a, Some(cat_b))
        .await
        .unwrap();
    for table in REQUIRED_RLS_TABLES {
        assert_eq!(org_rows(&mut tx, table, org_b).await, 0, "{table}");
    }
    tx.rollback().await.unwrap();
    assert!(store
        .get_dataset(&member_ctx(org_a, cat_b), "air")
        .await
        .unwrap()
        .is_none());
    assert!(store.get_catalogue(org_a, cat_b).await.unwrap().is_none());
}

#[tokio::test]
async fn a_harvest_run_cannot_write_into_another_organizations_catalogue() {
    let Some(pg) = pg("a_harvest_run_cannot_write_into_another_organizations_catalogue").await
    else {
        return;
    };
    let store = pg.app_store().await;
    let [(org_a, cat_a), (org_b, cat_b)] = split_two_organizations(&pg, &store).await;

    // A run of A's harvest that names B's tenant is refused by WITH CHECK.
    let mut tx = pg.app.begin().await.unwrap();
    set_tenant_config(&mut tx, org_a, Some(cat_a))
        .await
        .unwrap();
    let err = sqlx::query(
        "INSERT INTO dataroom.cat_dataset (org_id, catalogue_id, id, name, title, publisher_id,
                                           visibility, doc)
         VALUES ($1, $2, 'evil', 'evil', 'evil', 'city', 'public', '{}'::jsonb)",
    )
    .bind(org_b.as_uuid())
    .bind(cat_b.as_uuid())
    .execute(&mut *tx)
    .await
    .expect_err("WITH CHECK refuses a row of another Organization");
    assert_eq!(
        err.as_database_error().and_then(|d| d.code()).as_deref(),
        Some("42501"),
        "{err}"
    );
    tx.rollback().await.unwrap();

    // Through the store, A's principal aimed at B's catalogue id writes, at most, under A's own
    // OrgId (the store may also refuse it): B's catalogue gains neither a dataset nor a run.
    let stray = member_ctx(org_a, cat_b);
    let _written_or_refused = store
        .upsert_dataset(&stray, dataset("evil", "city", "public"))
        .await;
    let _written_or_refused = store
        .upsert_harvest_run(
            &stray,
            harvest_run("run-x", "hs-nkod", "2026-11-14T10:00:00Z"),
        )
        .await;
    let ctx_b = member_ctx(org_b, cat_b);
    assert!(store.get_dataset(&ctx_b, "evil").await.unwrap().is_none());
    assert!(store.get_dataset(&ctx_b, "air").await.unwrap().is_some());
    assert!(store
        .list_harvest_runs(&ctx_b, None)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn boot_check_refuses_superuser_and_bypassrls_roles() {
    let Some(pg) = pg("boot_check_refuses_superuser_and_bypassrls_roles").await else {
        return;
    };
    let refused = PgStore::connect_and_verify(pg.admin.clone()).await;
    assert!(
        matches!(&refused, Err(DataroomError::Storage(m)) if m.contains("SUPERUSER or BYPASSRLS")),
        "superuser: {:?}",
        refused.err()
    );

    sqlx::raw_sql(
        "DO $$ BEGIN
            IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'dataroom_bypass_test') THEN
                CREATE ROLE dataroom_bypass_test NOLOGIN NOSUPERUSER BYPASSRLS;
            END IF;
         END $$;",
    )
    .execute(&pg.admin)
    .await
    .unwrap();
    let bypass: PgPool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|conn, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE dataroom_bypass_test")
                    .execute(&mut *conn)
                    .await
                    .map(|_| ())
            })
        })
        .connect(&pg.url)
        .await
        .unwrap();
    let refused = verify_rls_on_pool(&bypass).await;
    assert!(
        matches!(&refused, Err(DataroomError::Storage(m)) if m.contains("BYPASSRLS")),
        "bypassrls: {refused:?}"
    );

    verify_rls_on_pool(&pg.app)
        .await
        .expect("request role passes");
}

#[tokio::test]
async fn migrate_is_idempotent_and_needs_the_owner() {
    let Some(pg) = pg("migrate_is_idempotent_and_needs_the_owner").await else {
        return;
    };
    let owner = PgStore::from_pool_unchecked(pg.admin.clone());
    owner.migrate().await.expect("second migration run");
    owner.migrate().await.expect("third migration run");
    assert!(owner.pool().size() >= 1);
    verify_rls_on_pool(&pg.app)
        .await
        .expect("forced RLS holds after re-running migrations");

    // The request role cannot run the migration (no CREATE on the database or schema).
    let err = PgStore::from_pool_unchecked(pg.app.clone())
        .migrate()
        .await
        .expect_err("the request role may not migrate");
    assert!(
        matches!(&err, DataroomError::Storage(m) if m.starts_with("migration failed")),
        "{err:?}"
    );
}

#[tokio::test]
async fn database_failures_surface_as_storage_errors() {
    let Some(pg) = pg("database_failures_surface_as_storage_errors").await else {
        return;
    };
    // A role with the schema but no table rights: every statement after the tenant is set fails.
    sqlx::raw_sql(
        "DO $$ BEGIN
            IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'dataroom_noaccess_test') THEN
                CREATE ROLE dataroom_noaccess_test NOLOGIN NOSUPERUSER NOBYPASSRLS;
            END IF;
         END $$;
         GRANT USAGE ON SCHEMA dataroom TO dataroom_noaccess_test;",
    )
    .execute(&pg.admin)
    .await
    .unwrap();
    let pool: PgPool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|conn, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE dataroom_noaccess_test")
                    .execute(&mut *conn)
                    .await
                    .map(|_| ())
            })
        })
        .connect(&pg.url)
        .await
        .unwrap();
    let store = PgStore::from_pool_unchecked(pool);
    let (org, cat) = tenant();
    let ctx = member_ctx(org, cat);
    let denied = |r: &str, e: Option<DataroomError>| match e {
        Some(DataroomError::Storage(m)) => assert!(m.contains("permission denied"), "{r}: {m}"),
        other => panic!("{r}: expected a storage error, got {other:?}"),
    };
    denied(
        "resolve_host",
        store.resolve_host("x.example.org").await.err(),
    );
    denied("list_catalogues", store.list_catalogues(org).await.err());
    denied("get_catalogue", store.get_catalogue(org, cat).await.err());
    denied(
        "upsert_catalogue",
        store
            .upsert_catalogue(catalogue(org, cat, "x", &unique_host("x")))
            .await
            .err(),
    );
    denied("list_publishers", store.list_publishers(&ctx).await.err());
    denied("get_publisher", store.get_publisher(&ctx, "p").await.err());
    denied(
        "upsert_publisher",
        store.upsert_publisher(&ctx, publisher("p")).await.err(),
    );
    denied(
        "merge_publishers",
        store.merge_publishers(&ctx, "a", "b").await.err(),
    );
    denied(
        "withdraw_publisher",
        store.withdraw_publisher(&ctx, "p").await.err(),
    );
    denied("list_datasets", store.list_datasets(&ctx).await.err());
    denied("get_dataset", store.get_dataset(&ctx, "d").await.err());
    denied(
        "upsert_dataset",
        store
            .upsert_dataset(&ctx, dataset("d", "p", "public"))
            .await
            .err(),
    );
    denied(
        "withdraw_dataset",
        store.withdraw_dataset(&ctx, "d").await.err(),
    );
    denied("purge_dataset", store.purge_dataset(&ctx, "d").await.err());
    denied(
        "get_distribution",
        store.get_distribution(&ctx, "r").await.err(),
    );
    denied(
        "list_resource_views",
        store.list_resource_views(&ctx, "r").await.err(),
    );
    denied(
        "get_resource_view",
        store.get_resource_view(&ctx, "v").await.err(),
    );
    denied(
        "upsert_resource_view",
        store
            .upsert_resource_view(&ctx, view("v", "r", 1))
            .await
            .err(),
    );
    denied(
        "delete_resource_view",
        store.delete_resource_view(&ctx, "v").await.err(),
    );
    denied(
        "list_harvest_sources",
        store.list_harvest_sources(&ctx).await.err(),
    );
    denied(
        "get_harvest_source",
        store.get_harvest_source(&ctx, "h").await.err(),
    );
    denied(
        "upsert_harvest_source",
        store
            .upsert_harvest_source(&ctx, harvest_source("h"))
            .await
            .err(),
    );
    denied(
        "list_harvest_runs",
        store.list_harvest_runs(&ctx, None).await.err(),
    );
    denied(
        "list_harvest_runs by source",
        store.list_harvest_runs(&ctx, Some("h")).await.err(),
    );
    denied(
        "get_harvest_run",
        store.get_harvest_run(&ctx, "r").await.err(),
    );
    denied(
        "upsert_harvest_run",
        store
            .upsert_harvest_run(&ctx, harvest_run("r", "h", "2026-11-01T10:00:00Z"))
            .await
            .err(),
    );
    denied(
        "record_activity",
        store
            .record_activity(&ctx, activity("a", "o", "2026-11-01T10:00:00Z"))
            .await
            .err(),
    );
    denied(
        "list_activity",
        store.list_activity(&ctx, None, 5).await.err(),
    );
    denied(
        "list_activity by object",
        store.list_activity(&ctx, Some("o"), 5).await.err(),
    );
    denied(
        "upsert_shacl_report",
        store
            .upsert_shacl_report(&ctx, shacl_report("d", "p", true))
            .await
            .err(),
    );
    denied(
        "list_shacl_reports",
        store.list_shacl_reports(&ctx, None, None).await.err(),
    );
    denied("list_profiles", store.list_profiles(&ctx).await.err());
    denied(
        "upsert_profile",
        store.upsert_profile(&ctx, profile("p", "1")).await.err(),
    );
    denied("list_pages", store.list_pages(&ctx).await.err());
    denied(
        "upsert_page",
        store.upsert_page(&ctx, page("a", "sk")).await.err(),
    );
    denied("list_plugins", store.list_plugins(&ctx).await.err());
    denied(
        "upsert_plugin",
        store.upsert_plugin(&ctx, plugin("m")).await.err(),
    );
    for set in [
        "Catalogues",
        "Datasets",
        "Distributions",
        "Publishers",
        "HarvestSources",
        "HarvestRuns",
        "ActivityRows",
        "ShaclResults",
    ] {
        denied(set, store.odata_table(&ctx, set).await.err());
    }
    let boot = verify_rls_on_pool(store.pool()).await;
    assert!(boot.is_ok(), "the role itself is not privileged: {boot:?}");

    // A closed pool: every call fails before it reaches a statement.
    store.pool().close().await;
    for err in [
        store.list_datasets(&ctx).await.err(),
        store.resolve_host("x").await.err(),
        store.migrate().await.err(),
        verify_rls_on_pool(store.pool()).await.err(),
    ] {
        assert!(matches!(err, Some(DataroomError::Storage(_))), "{err:?}");
    }
}
