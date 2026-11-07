// SPDX-License-Identifier: EUPL-1.2
//! A rollback boots the previous release on the schema a newer release
//! migrated (T-5105, docs/src/operations.md). Needs a live PostGIS; skips when
//! ANTARES_TEST_DATABASE_URL is unset, like tests/pg.rs. It writes rows into
//! `_sqlx_migrations`, which every pg test reads at connect, so it runs in a
//! database of its own: in the shared one a concurrent test's connect met the
//! seeded rows (VersionTooNew(18, 16), train 20261022-0032 fast 39701518856).

#![cfg(feature = "postgres")]

use antares_sql::store::pg;

#[tokio::test(flavor = "multi_thread")]
async fn a_newer_compatible_migration_boots_and_a_breaking_one_stops_the_boot() {
    let Ok(shared) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return;
    };
    let admin = pg::connect(&shared, 2).await.expect("connect+migrate");
    let run = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis();
    let name = format!("antares_rollback_{run}");
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .execute(&admin)
        .await
        .expect("own database");
    let (base, _) = shared.rsplit_once('/').expect("a database URL");
    let url = format!("{base}/{name}");
    let pool = pg::connect(&url, 2)
        .await
        .expect("connect+migrate the own database");
    let newest = pg::MIGRATOR
        .iter()
        .map(|m| m.version)
        .max()
        .expect("migrations");
    let future = |v: i64, d: &'static str| {
        sqlx::query(
            "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
             VALUES ($1, $2, true, '\\x00', 0)",
        )
        .bind(v)
        .bind(d)
    };
    let clean = || sqlx::query("DELETE FROM _sqlx_migrations WHERE version > $1").bind(newest);
    clean().execute(&pool).await.expect("clean");

    future(newest + 1, "type counts")
        .execute(&pool)
        .await
        .expect("seed compatible");
    let compatible = pg::connect(&url, 2)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string());

    future(newest + 2, "breaking drop names")
        .execute(&pool)
        .await
        .expect("seed breaking");
    let breaking = pg::connect(&url, 2)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string());
    let permanent = match pg::connect(&url, 2).await {
        Err(e) => pg::is_schema_mismatch(&e),
        Ok(_) => false,
    };
    clean().execute(&pool).await.expect("clean");

    // F-ANT1-012: a compatible newer version beside an applied older version
    // this binary never shipped (version 0) still fails the boot.
    future(newest + 1, "type counts")
        .execute(&pool)
        .await
        .expect("seed compatible");
    future(0, "another release")
        .execute(&pool)
        .await
        .expect("seed unknown older");
    let unknown_older = pg::connect(&url, 2)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string());
    clean().execute(&pool).await.expect("clean");
    sqlx::query("DELETE FROM _sqlx_migrations WHERE version = 0")
        .execute(&pool)
        .await
        .expect("clean older");
    assert_eq!(
        unknown_older,
        Err(
            "migration 0 was previously applied but is missing in the resolved migrations"
                .to_owned()
        )
    );

    assert_eq!(compatible, Ok(()));
    assert_eq!(
        breaking,
        Err(format!(
            "migration {} is newer than the latest applied migration {newest}",
            newest + 2
        ))
    );
    assert!(
        permanent,
        "a breaking schema is no reason to retry the boot"
    );
}

/// T-5200: migration 0020 is `compatible` because it leaves the cut-over
/// unset: until a later release sets it, every history row is written to
/// `attr_instances`, the only parent the release before 0020 reads, so a
/// rollback across 0020 loses no row.
#[tokio::test(flavor = "multi_thread")]
async fn the_integer_keys_migration_leaves_the_cut_over_unset() {
    let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return;
    };
    let pool = pg::connect(&url, 2).await.expect("connect+migrate");
    let rows: Vec<Option<String>> = sqlx::query_scalar("SELECT at::text FROM history_cutover")
        .fetch_all(&pool)
        .await
        .expect("cut-over");
    assert_eq!(rows, vec![None], "one row, no cut-over");
    let m = pg::MIGRATOR
        .iter()
        .find(|m| m.version == 20)
        .expect("migration 0020");
    assert!(
        !m.description.starts_with("breaking "),
        "0020 must stay compatible: {}",
        m.description
    );
}

/// T-5249: 0023 puts the short history in an unlogged child of `attr_rows`
/// (plain mode) and is `breaking`, because its unique key takes `short` and a
/// 0022 release's ON CONFLICT names a key no index has.
#[tokio::test(flavor = "multi_thread")]
async fn short_history_is_unlogged_and_0023_breaking() {
    let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return;
    };
    let pool = pg::connect(&url, 2).await.expect("connect+migrate");
    let timescale: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'timescaledb')",
    )
    .fetch_one(&pool)
    .await
    .expect("extension");
    let kinds: Vec<(String, String)> = sqlx::query_as(
        "SELECT relname::text, relpersistence::text FROM pg_class
          WHERE relname IN ('attr_rows_short', 'attr_rows_long')
            AND relnamespace = 'public'::regnamespace ORDER BY relname",
    )
    .fetch_all(&pool)
    .await
    .expect("persistence");
    let want: Vec<(String, String)> = if timescale {
        Vec::new()
    } else {
        vec![
            ("attr_rows_long".into(), "p".into()),
            ("attr_rows_short".into(), "u".into()),
        ]
    };
    assert_eq!(
        kinds, want,
        "the short child is unlogged, the long one logged"
    );
    let breaking = pg::MIGRATOR
        .iter()
        .find(|m| m.version == 23)
        .map(|m| m.description.starts_with("breaking "));
    assert_eq!(breaking, Some(true), "0023 is breaking");
}
