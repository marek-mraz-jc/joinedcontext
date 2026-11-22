#![forbid(unsafe_code)]
#![allow(clippy::disallowed_methods)] // Tests read their database URL; jc runs none of this.
//! T-7509: the installation's organizations on Postgres: an earlier release's database keeps
//! its numbers, ids and rows, and two replicas give one number.
//!
//! Runs when `DAIYA_TEST_DATABASE_URL` is set (printing a `SKIP` line when unset).

use daiya_model::{DaiyaStore, InstallationId, OrgId, OrgKey};
use daiya_store_pg::{MIGRATION_SQL, PgStore, connect_test_database, migrate};
use sqlx::AssertSqlSafe;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::str::FromStr;

#[tokio::test]
async fn a_database_of_the_release_before_keeps_its_numbers_ids_and_rows() {
    let Ok(url) = std::env::var("DAIYA_TEST_DATABASE_URL") else {
        println!("SKIP: DAIYA_TEST_DATABASE_URL is unset");
        return;
    };
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
            .create_org(installation, "kosice", Some(3), OrgId::new())
            .await
            .is_err(),
        "a held number is never renumbered"
    );

    let (a, b) = tokio::join!(
        first.create_org(installation, "kosice", None, OrgId::new()),
        second.create_org(installation, "nitra", None, OrgId::new()),
    );
    let mut given = [a.unwrap().number, b.unwrap().number];
    given.sort_unstable();
    assert_eq!(given, [8, 9], "the database numbers");
    let (a, b) = tokio::join!(
        first.create_org(installation, "trnava", None, OrgId::new()),
        second.create_org(installation, "trnava", None, OrgId::new()),
    );
    assert_eq!(
        usize::from(a.is_ok()) + usize::from(b.is_ok()),
        1,
        "one name, one row"
    );

    assert!(second.retire_org(installation, 3).await.unwrap());
    let again = first
        .create_org(installation, "helsinki", None, helsinki)
        .await
        .unwrap();
    assert_ne!(
        again.org_id, helsinki,
        "a name created again never owns the old rows"
    );
    assert_eq!(again.number, 11);
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
