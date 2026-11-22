#![forbid(unsafe_code)]
#![allow(clippy::disallowed_methods)] // Tests read their database URL; jc runs none of this.
//! T-7509: the installation's organizations on Postgres: an earlier release's database keeps
//! its numbers, ids and rows, and two replicas give one number.
//!
//! Runs when `DAIYA_TEST_DATABASE_URL` is set (printing a `SKIP` line when unset).

use daiya_model::{
    DaiyaStore, ImportReportRecord, InstallationId, InstanceId, OrgId, OrgKey, OrgQuotas,
};
use daiya_store_pg::{MIGRATION_SQL, PgStore, connect_test_database, migrate};
use sqlx::AssertSqlSafe;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::str::FromStr;

/// The roles are the cluster's, so this file's databases migrate one at a time.
static CLUSTER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn a_database_of_the_release_before_keeps_its_numbers_ids_and_rows() {
    let Ok(url) = std::env::var("DAIYA_TEST_DATABASE_URL") else {
        println!("SKIP: DAIYA_TEST_DATABASE_URL is unset");
        return;
    };
    let _cluster = CLUSTER.lock().await;
    let server = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("DAIYA_TEST_DATABASE_URL is set but unreachable");
    let name = format!("daiya_orgs_{}", uuid::Uuid::new_v4().simple());
    sqlx::raw_sql(AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .execute(&server)
        .await
        .expect("create test database");
    let options = PgConnectOptions::from_str(&url)
        .expect("url")
        .database(&name);
    let owner = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .expect("connect test database");

    // What the release before left: version 1 alone, and a project's rows under the id it
    // derived from the name.
    sqlx::raw_sql(MIGRATION_SQL)
        .execute(&owner)
        .await
        .expect("version 1");
    let installation = InstallationId::from_u128(1);
    let (helsinki, zilina) = (OrgId::from_u128(0xa1), OrgId::from_u128(0xb2));
    sqlx::query(
        "INSERT INTO cr_quota (installation_id, org_id, instance_id, max_dashboards) VALUES ($1, $2, $1, 7)",
    )
    .bind(installation.as_uuid())
    .bind(helsinki.as_uuid())
    .execute(&owner)
    .await
    .expect("a row of the release before");

    migrate(&owner).await.expect("migrate");
    migrate(&owner).await.expect("a second run changes nothing");
    let mut db_url = url::Url::parse(&url).expect("url");
    db_url.set_path(&name);
    let db = connect_test_database(db_url.as_str())
        .await
        .expect("app store");
    db.app
        .verify_rls_enforced()
        .await
        .expect("the boot check holds");

    // The exported numbers are declared; they and the ids are today's, on every replica.
    let first = &db.app;
    let second = PgStore::new(db.app_config.connect().await.expect("second replica"));
    for store in [first, &second] {
        let zil = store
            .create_org(installation, "zilina", Some(7), zilina)
            .await
            .unwrap();
        let hel = store
            .create_org(installation, "helsinki", Some(3), helsinki)
            .await
            .unwrap();
        assert_eq!(
            (hel.number, hel.org_id, zil.number, zil.org_id),
            (3, helsinki, 7, zilina)
        );
    }
    let kept = second.get_quotas(helsinki).await.unwrap();
    assert_eq!(
        kept.max_dashboards, 7,
        "its rows are found under the same id"
    );
    assert!(
        second
            .create_org(installation, "kosice", Some(3), OrgId::generate())
            .await
            .is_err(),
        "a held number is never renumbered"
    );

    let (a, b) = tokio::join!(
        first.create_org(installation, "kosice", None, OrgId::generate()),
        second.create_org(installation, "nitra", None, OrgId::generate()),
    );
    let mut given = [a.unwrap().number, b.unwrap().number];
    given.sort_unstable();
    assert_eq!(given, [8, 9], "the database numbers");
    let (a, b) = tokio::join!(
        first.create_org(installation, "trnava", None, OrgId::generate()),
        second.create_org(installation, "trnava", None, OrgId::generate()),
    );
    assert_eq!(
        usize::from(a.is_ok()) + usize::from(b.is_ok()),
        1,
        "one name, one row"
    );

    assert!(second.retire_org(installation, 3).await.unwrap());
    let named = || first.find_org(installation, OrgKey::Name("helsinki".into()));
    let gone = named()
        .await
        .unwrap()
        .expect("a retired name is still known");
    assert!(gone.retired && gone.org_id == helsinki);
    let again = first
        .create_org(installation, "helsinki", None, helsinki)
        .await
        .unwrap();
    assert_ne!(
        again.org_id, helsinki,
        "a name created again never owns the old rows"
    );
    assert_eq!(again.number, 11);
    assert_eq!(named().await.unwrap(), Some(again.clone()), "the live one");
    assert_eq!(
        first.get_quotas(again.org_id).await.unwrap(),
        Default::default()
    );
    let old = first
        .find_org(installation, OrgKey::Id(helsinki))
        .await
        .unwrap()
        .unwrap();
    assert!(old.retired && old.number == 3);
    assert_eq!(first.list_orgs(installation).await.unwrap().len(), 5);
    let elsewhere = InstallationId::from_u128(2);
    assert!(first.list_orgs(elsewhere).await.unwrap().is_empty());

    // The application role neither reads nor writes the table itself.
    for sql in ["SELECT name FROM daiya_org", "DELETE FROM daiya_org"] {
        assert!(
            sqlx::query(sql).execute(first.pool()).await.is_err(),
            "{sql}"
        );
    }

    first.pool().close().await;
    second.pool().close().await;
    db.admin.close().await;
    owner.close().await;
    sqlx::raw_sql(AssertSqlSafe(format!("DROP DATABASE {name} WITH (FORCE)")))
        .execute(&server)
        .await
        .expect("drop test database");
}

/// Postgres searches the session's temporary schema first unless the path names it, and the
/// application role may create there: an owner-run function must name it last.
#[tokio::test]
async fn every_definer_function_searches_the_temporary_schema_last() {
    let Ok(url) = std::env::var("DAIYA_TEST_DATABASE_URL") else {
        println!("SKIP: DAIYA_TEST_DATABASE_URL is unset");
        return;
    };
    let _cluster = CLUSTER.lock().await;
    let db = connect_test_database(&url).await.expect("test database");
    // An extension's functions are its own.
    let definers: Vec<(String, bool)> = sqlx::query_as(
        "SELECT p.oid::regprocedure::text, EXISTS (SELECT 1 FROM unnest(p.proconfig) c
             WHERE c ~ '^search_path=pg_catalog,.* pg_temp$')
         FROM pg_proc p
         WHERE p.prosecdef
           AND p.pronamespace = (SELECT relnamespace FROM pg_class WHERE oid = 'daiya_org'::regclass)
           AND NOT EXISTS (SELECT 1 FROM pg_depend d
                           WHERE d.classid = 'pg_proc'::regclass AND d.objid = p.oid
                             AND d.deptype = 'e')",
    )
    .fetch_all(&db.admin)
    .await
    .expect("the catalog");
    assert!(definers.len() >= 5, "{definers:?}");
    let open: Vec<&String> = definers.iter().filter(|f| !f.1).map(|f| &f.0).collect();
    assert!(open.is_empty(), "pg_temp is not last in {open:?}");
}

#[tokio::test]
async fn a_temporary_table_of_the_caller_changes_no_definer_functions_result() {
    let Ok(url) = std::env::var("DAIYA_TEST_DATABASE_URL") else {
        println!("SKIP: DAIYA_TEST_DATABASE_URL is unset");
        return;
    };
    let _cluster = CLUSTER.lock().await;
    let db = connect_test_database(&url).await.expect("test database");
    let installation = InstallationId::generate();
    let (org, forged) = (OrgId::generate(), OrgId::generate());
    db.app
        .create_org(installation, "real", None, org)
        .await
        .unwrap();

    let mut caller = db.app.pool().acquire().await.expect("a session");
    sqlx::raw_sql(
        "CREATE TEMP TABLE daiya_org (installation_id UUID, number BIGINT, name TEXT,
             org_id UUID, created_at TIMESTAMPTZ DEFAULT now(), retired_at TIMESTAMPTZ);
         CREATE TEMP TABLE daiya_public_token (access_token_hash TEXT PRIMARY KEY, org_id UUID);
         GRANT ALL ON pg_temp.daiya_org, pg_temp.daiya_public_token TO PUBLIC",
    )
    .execute(&mut *caller)
    .await
    .expect("the caller's own tables");
    sqlx::query(
        "INSERT INTO pg_temp.daiya_org (installation_id, number, name, org_id)
         VALUES ($1, 2, 'forged', $2)",
    )
    .bind(installation.as_uuid())
    .bind(forged.as_uuid())
    .execute(&mut *caller)
    .await
    .expect("a forged row");
    let hash = uuid::Uuid::new_v4().simple().to_string();
    sqlx::query("INSERT INTO pg_temp.daiya_public_token VALUES ($1, $2)")
        .bind(&hash)
        .bind(forged.as_uuid())
        .execute(&mut *caller)
        .await
        .expect("a forged token");

    let found: Vec<uuid::Uuid> =
        sqlx::query_scalar("SELECT org_id FROM daiya_org_find($1, NULL, NULL, NULL)")
            .bind(installation.as_uuid())
            .fetch_all(&mut *caller)
            .await
            .unwrap();
    assert_eq!(found, [org.as_uuid()], "find");
    let token_org: Option<uuid::Uuid> = sqlx::query_scalar("SELECT daiya_public_token_org($1)")
        .bind(&hash)
        .fetch_one(&mut *caller)
        .await
        .unwrap();
    assert_eq!(token_org, None, "the token index");

    let added = OrgId::generate();
    sqlx::query("SELECT number FROM daiya_org_create($1, 'added', NULL, $2, $2)")
        .bind(installation.as_uuid())
        .bind(added.as_uuid())
        .fetch_all(&mut *caller)
        .await
        .unwrap();
    let retired: bool = sqlx::query_scalar("SELECT daiya_org_retire($1, 2)")
        .bind(installation.as_uuid())
        .fetch_one(&mut *caller)
        .await
        .unwrap();
    assert!(retired);
    let rows: Vec<(i64, uuid::Uuid, bool)> = sqlx::query_as(
        "SELECT number, org_id, retired_at IS NOT NULL FROM public.daiya_org
         WHERE installation_id = $1 ORDER BY number",
    )
    .bind(installation.as_uuid())
    .fetch_all(&db.admin)
    .await
    .unwrap();
    assert_eq!(
        rows,
        [(2, org.as_uuid(), true), (3, added.as_uuid(), false)],
        "create and retire"
    );

    sqlx::query("SELECT set_config('cr.org', $1, false)")
        .bind(added.to_string())
        .execute(&mut *caller)
        .await
        .unwrap();
    let shared = uuid::Uuid::new_v4().simple().to_string();
    sqlx::query(
        "INSERT INTO cr_public_dashboard
             (installation_id, org_id, instance_id, uid, dashboard_uid, access_token_hash)
         VALUES ($1, $2, $1, 'shadow', 'shadow', $3)",
    )
    .bind(installation.as_uuid())
    .bind(added.as_uuid())
    .bind(&shared)
    .execute(&mut *caller)
    .await
    .expect("a share");
    let indexed: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT org_id FROM public.daiya_public_token WHERE access_token_hash = $1",
    )
    .bind(&shared)
    .fetch_optional(&db.admin)
    .await
    .unwrap();
    assert_eq!(indexed, Some(added.as_uuid()), "the trigger");
    caller
        .close()
        .await
        .expect("the session and its tables end");
}

#[tokio::test]
async fn a_quota_and_an_import_report_keep_the_installation_they_were_written_for() {
    let Ok(url) = std::env::var("DAIYA_TEST_DATABASE_URL") else {
        println!("SKIP: DAIYA_TEST_DATABASE_URL is unset");
        return;
    };
    let _cluster = CLUSTER.lock().await;
    let db = connect_test_database(&url).await.expect("test database");
    let (installation, org, instance) = (
        InstallationId::from_u128(9),
        OrgId::generate(),
        InstanceId::from_u128(8),
    );
    db.app
        .set_quotas(installation, org, instance, OrgQuotas::default())
        .await
        .unwrap();
    db.app
        .insert_import_report(
            installation,
            ImportReportRecord {
                id: "r".into(),
                org_id: org,
                instance_id: instance,
                dry_run: false,
                status: "ok".into(),
                dashboard_count: 1,
                refused_count: 0,
                mapped_count: 0,
                summary: serde_json::json!({}),
                created_at: jc_time::TimestampTz::now(),
            },
        )
        .await
        .unwrap();
    for table in ["cr_quota", "cr_import_report"] {
        let stored: (uuid::Uuid, uuid::Uuid) = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT installation_id, instance_id FROM {table} WHERE org_id = $1"
        )))
        .bind(org.as_uuid())
        .fetch_one(&db.admin)
        .await
        .unwrap();
        assert_eq!(
            stored,
            (installation.as_uuid(), instance.as_uuid()),
            "{table}"
        );
    }
}
