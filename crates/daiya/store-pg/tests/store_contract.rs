#![forbid(unsafe_code)]
#![allow(clippy::disallowed_methods)] // Tests read their database URL and run the binaries they test; jc runs none of this.
//! The behaviour contract every `DaiyaStore` holds (shared with daiya-store-memory's tests, which
//! run it on the memory store), here on the Postgres store (on the application login role, so
//! forced RLS applies) when `DAIYA_TEST_DATABASE_URL` is set, with a `SKIP` line otherwise (R8).
//! Postgres-only checks follow: RLS seen through raw SQL, the public-token policy, the boot
//! check, the pool configuration and persistence.

#[path = "../../store-memory/tests/contract/mod.rs"]
mod contract;
use contract::*;

use daiya_model::{
    DaiyaStore, ImportReportRecord, InstallationId, LibraryPanelRecord, OrgId, OrgQuotas,
    sha256_hex,
};
use daiya_store_pg::{
    PgPoolConfig, PgStore, TestDatabase, connect_test_database, parse_url_without_password,
};
use jc_time::TimestampTz;
use serde_json::json;
use sqlx::Row;
use tokio::sync::RwLock;

/// Tests that create a table in the shared schema take this lock for writing, every other
/// Postgres test for reading, so a rogue table never shows up in another test's boot check.
static SCHEMA_LOCK: RwLock<()> = RwLock::const_new(());

async fn pg() -> Option<TestDatabase> {
    let Ok(url) = std::env::var("DAIYA_TEST_DATABASE_URL") else {
        println!("SKIP: DAIYA_TEST_DATABASE_URL is unset");
        return None;
    };
    Some(connect_test_database(&url).await.expect("test database"))
}

#[tokio::test]
async fn store_contract_holds_on_postgres_through_the_app_role() {
    let _schema = SCHEMA_LOCK.read().await;
    let Some(db) = pg().await else { return };
    run_contract(&db.app).await;
}

/// Runs `sql` in one transaction of the application role with `cr.org` (and optionally
/// `cr.public_token`) set, answering the number of rows the role can see.
async fn visible_rows(
    db: &TestDatabase,
    table: &str,
    org: Option<OrgId>,
    public_token_hash: Option<&str>,
) -> i64 {
    let mut tx = db.app.pool().begin().await.unwrap();
    if let Some(org) = org {
        sqlx::query("SELECT set_config('cr.org', $1, true)")
            .bind(org.to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    if let Some(hash) = public_token_hash {
        sqlx::query("SELECT set_config('cr.public_token', $1, true)")
            .bind(hash)
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    let n: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}")))
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    n
}

#[tokio::test]
async fn forced_rls_hides_other_organizations_from_raw_sql_of_the_app_role() {
    let _schema = SCHEMA_LOCK.read().await;
    let Some(db) = pg().await else { return };
    let (org_a, org_b) = (OrgId::generate(), OrgId::generate());
    for org in [org_a, org_b] {
        db.app
            .upsert_dashboard(dashboard(org, "same-uid", None), "", true)
            .await
            .unwrap();
        db.app
            .upsert_datasource(datasource(org, "prom", "Prometheus", "prometheus"))
            .await
            .unwrap();
    }
    db.app
        .upsert_dashboard(dashboard(org_b, "only-b", None), "", true)
        .await
        .unwrap();

    // No WHERE clause: the policy alone narrows to the transaction's organization.
    assert_eq!(
        visible_rows(&db, "cr_dashboard", Some(org_a), None).await,
        1
    );
    assert_eq!(
        visible_rows(&db, "cr_dashboard", Some(org_b), None).await,
        2
    );
    assert_eq!(
        visible_rows(&db, "cr_dashboard_version", Some(org_b), None).await,
        2
    );
    assert_eq!(
        visible_rows(&db, "cr_datasource", Some(org_a), None).await,
        1
    );
    // Without an organization the role sees nothing at all.
    for table in daiya_store_pg::REQUIRED_RLS_TABLES {
        assert_eq!(visible_rows(&db, table, None, None).await, 0, "{table}");
    }

    // Writing a row of B while the transaction is A's breaks the WITH CHECK clause, and an
    // UPDATE or DELETE aimed at B's rows touches nothing.
    let mut tx = db.app.pool().begin().await.unwrap();
    sqlx::query("SELECT set_config('cr.org', $1, true)")
        .bind(org_a.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let updated = sqlx::query("UPDATE cr_dashboard SET title = 'pwned' WHERE org_id = $1")
        .bind(org_b.as_uuid())
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(updated.rows_affected(), 0);
    let deleted = sqlx::query("DELETE FROM cr_datasource WHERE org_id = $1")
        .bind(org_b.as_uuid())
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(deleted.rows_affected(), 0);
    let insert = sqlx::query(
        "INSERT INTO cr_folder (installation_id, org_id, instance_id, uid, title) VALUES ($1, $2, $3, 'x', 'x')",
    )
    .bind(INST.as_uuid())
    .bind(org_b.as_uuid())
    .bind(INSTANCE.as_uuid())
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert!(
        insert.to_string().contains("row-level security"),
        "{insert}"
    );
    drop(tx);
    assert_eq!(
        db.app
            .get_dashboard(org_b, "same-uid")
            .await
            .unwrap()
            .unwrap()
            .title,
        "Dash same-uid"
    );
}

#[tokio::test]
async fn a_public_token_resolves_through_the_lookup_and_no_setting_widens_rls() {
    let _schema = SCHEMA_LOCK.read().await;
    let Some(db) = pg().await else { return };
    let org = OrgId::generate();
    let other = OrgId::generate();
    let token = format!("pub-{}", uuid::Uuid::new_v4());
    let hash = sha256_hex(token.as_bytes());
    for (o, uid, tok) in [
        (org, "pd-1", token.clone()),
        (other, "pd-2", format!("x-{token}")),
    ] {
        db.app
            .upsert_dashboard(dashboard(o, "pub-dash", None), "", true)
            .await
            .unwrap();
        db.app
            .upsert_public_dashboard(public_dashboard(o, uid, "pub-dash", &tok))
            .await
            .unwrap();
    }

    // No session setting other than `cr.org` shows a row: the old token setting sees nothing.
    assert_eq!(
        visible_rows(&db, "cr_public_dashboard", None, Some(&hash)).await,
        0
    );
    assert_eq!(
        visible_rows(&db, "cr_dashboard", None, Some(&hash)).await,
        0
    );

    // The application role cannot list the token index; it resolves one known hash to its
    // organization through the lookup function only.
    let listed = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM daiya_public_token")
        .fetch_one(db.app.pool())
        .await;
    assert!(
        listed.is_err(),
        "the token index is not readable by the app role"
    );
    let resolve = |h: String| {
        let pool = db.app.pool().clone();
        async move {
            sqlx::query_scalar::<_, Option<uuid::Uuid>>("SELECT daiya_public_token_org($1)")
                .bind(h)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(resolve(hash.clone()).await, Some(org.0));
    assert_eq!(resolve("not-a-hash".into()).await, None);
    assert_eq!(resolve(String::new()).await, None);

    // Through the store: the anonymous lookup returns the dashboard of the right organization.
    let (pd, dash) = db
        .app
        .get_public_dashboard_by_token_hash(&hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((pd.org_id, dash.org_id), (org, org));
    assert!(!pd.show_query_text);

    // A second public dashboard for the same dashboard is a conflict, not a 500.
    let dup = db
        .app
        .upsert_public_dashboard(public_dashboard(org, "pd-dup", "pub-dash", "dup-token"))
        .await
        .unwrap_err();
    assert_eq!(dup.status_code(), 409);
}

#[tokio::test]
async fn data_survives_a_new_store_instance() {
    let _schema = SCHEMA_LOCK.read().await;
    let Some(db) = pg().await else { return };
    let org = OrgId::generate();
    db.app
        .upsert_dashboard(dashboard(org, "durable", None), "first", true)
        .await
        .unwrap();
    db.app
        .upsert_datasource(datasource(org, "durable-ds", "Durable", "prometheus"))
        .await
        .unwrap();
    db.app.pool().close().await;

    let reopened = PgStore::new(db.app_config.connect().await.unwrap());
    reopened.verify_rls_enforced().await.unwrap();
    let dash = reopened
        .get_dashboard(org, "durable")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(dash.version, 1);
    assert_eq!(dash.data["uid"], "durable");
    assert_eq!(
        reopened
            .list_dashboard_versions(org, "durable")
            .await
            .unwrap()[0]
            .message,
        "first"
    );
    assert_eq!(
        reopened
            .get_datasource(org, "durable-ds")
            .await
            .unwrap()
            .unwrap()
            .secret_refs["token"],
        "prom-token"
    );
}

#[tokio::test]
async fn boot_check_refuses_superuser_set_role_and_unforced_tables() {
    let _schema = SCHEMA_LOCK.write().await;
    let Some(db) = pg().await else { return };
    // The application role passes.
    db.app.verify_rls_enforced().await.unwrap();

    // The migrating superuser does not.
    let admin_store = PgStore::new(db.admin.clone());
    let err = admin_store.verify_rls_enforced().await.unwrap_err();
    assert_eq!(err.slug(), "cr.rls_not_enforced");

    // Nor does a superuser login that switched to the application role: it could switch back.
    let url = std::env::var("DAIYA_TEST_DATABASE_URL").unwrap();
    let set_role_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|conn, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE daiya_app")
                    .execute(&mut *conn)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    let current: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(&set_role_pool)
        .await
        .unwrap();
    assert_eq!(current, "daiya_app");
    let err = PgStore::new(set_role_pool)
        .verify_rls_enforced()
        .await
        .unwrap_err();
    assert!(err.to_string().contains("SUPERUSER") || err.to_string().contains("BYPASSRLS"));

    // A cr_* table without forced RLS is refused by name, for the application role too.
    sqlx::query("CREATE TABLE IF NOT EXISTS cr_test_unforced (org_id UUID)")
        .execute(&db.admin)
        .await
        .unwrap();
    let unforced = db.app.verify_rls_enforced().await;
    sqlx::query("ALTER TABLE cr_test_unforced ENABLE ROW LEVEL SECURITY")
        .execute(&db.admin)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE cr_test_unforced FORCE ROW LEVEL SECURITY")
        .execute(&db.admin)
        .await
        .unwrap();
    let no_policy = db.app.verify_rls_enforced().await;
    sqlx::query("DROP TABLE IF EXISTS cr_test_unforced")
        .execute(&db.admin)
        .await
        .unwrap();
    let err = unforced.unwrap_err();
    assert!(err.to_string().contains("cr_test_unforced"), "{err}");
    let err = no_policy.unwrap_err();
    assert!(err.to_string().contains("cr_test_unforced"), "{err}");
    db.app.verify_rls_enforced().await.unwrap();
}

#[tokio::test]
async fn migration_is_idempotent_and_runs_concurrently() {
    let _schema = SCHEMA_LOCK.write().await;
    let Some(db) = pg().await else { return };
    // A recorded checksum that no longer matches makes the next boot re-run the DDL once.
    sqlx::query("UPDATE daiya_schema_migrations SET checksum = 'stale' WHERE version = 1")
        .execute(&db.admin)
        .await
        .unwrap();
    let a = daiya_store_pg::migrate(&db.admin);
    let b = daiya_store_pg::migrate(&db.admin);
    let (ra, rb) = tokio::join!(a, b);
    ra.unwrap();
    rb.unwrap();
    let policies: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pg_policy p JOIN pg_class c ON c.oid = p.polrelid WHERE c.relname = 'cr_public_dashboard'",
    )
    .fetch_one(&db.admin)
    .await
    .unwrap();
    assert_eq!(
        policies, 1,
        "org_isolation only: no second setting widens RLS"
    );
    let path: String = sqlx::query_scalar(
        "SELECT array_to_string(proconfig, ',') FROM pg_proc
         WHERE oid = 'daiya_public_token_org(text)'::regprocedure",
    )
    .fetch_one(&db.admin)
    .await
    .unwrap();
    assert!(
        path.ends_with(", pg_temp"),
        "version 1 again never undoes what version 2 altered: {path}"
    );
    let checksum: String =
        sqlx::query_scalar("SELECT checksum FROM daiya_schema_migrations WHERE version = 1")
            .fetch_one(&db.admin)
            .await
            .unwrap();
    assert_eq!(
        checksum,
        sha256_hex(daiya_store_pg::MIGRATION_SQL.as_bytes())
    );
    let app_role =
        sqlx::query("SELECT rolcanlogin, rolbypassrls FROM pg_roles WHERE rolname = 'daiya_app'")
            .fetch_one(&db.admin)
            .await
            .unwrap();
    assert!(!app_role.get::<bool, _>("rolbypassrls"));
}

#[tokio::test]
async fn pool_config_reads_the_password_file_and_refuses_passwords_in_urls() {
    let dir = std::env::temp_dir().join(format!("daiya-pgcfg-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let pw = dir.join("pw");
    std::fs::write(&pw, "  s3cret\n").unwrap();
    let empty = dir.join("empty");
    std::fs::write(&empty, "\n").unwrap();

    let cfg = PgPoolConfig::from_url_and_secret_file(
        "postgres://daiya_app_login@db.internal:6543/dash",
        &pw,
        0,
    )
    .await
    .unwrap();
    assert_eq!(
        (
            cfg.host.as_str(),
            cfg.port,
            cfg.database.as_str(),
            cfg.app_role.as_str()
        ),
        ("db.internal", 6543, "dash", "daiya_app_login")
    );
    assert_eq!(cfg.password.as_str(), "s3cret");
    assert_eq!(cfg.max_connections, 1);
    // The password never shows in Debug output of anything holding the URL.
    assert!(
        !format!(
            "{:?}",
            parse_url_without_password("postgres://u@h/d").unwrap()
        )
        .contains("s3cret")
    );

    for bad in [
        "postgres://user:inline@db/dash",
        "mysql://user@db/dash",
        "postgres://db/dash",
        "not a url",
    ] {
        let err = PgPoolConfig::from_url_and_secret_file(bad, &pw, 4)
            .await
            .unwrap_err();
        assert_eq!(err.status_code(), 400, "{bad}: {err}");
        assert!(!err.to_string().contains("inline"), "{err}");
    }
    let err = PgPoolConfig::from_secret_file("h", 5432, "d", "r", &empty, 2)
        .await
        .unwrap_err();
    assert_eq!(err.status_code(), 400);
    let err = PgPoolConfig::from_secret_file("h", 5432, "d", "r", dir.join("absent"), 2)
        .await
        .unwrap_err();
    assert_eq!(err.status_code(), 500);
    let ok = PgPoolConfig::from_secret_file("h", 5432, "d", "r", &pw, 3)
        .await
        .unwrap();
    assert_eq!((ok.max_connections, ok.password.as_str()), (3, "s3cret"));
    std::fs::remove_dir_all(&dir).unwrap();

    // A wrong password does not connect, and the error does not echo it.
    if let Ok(url) = std::env::var("DAIYA_TEST_DATABASE_URL") {
        let opts: sqlx::postgres::PgConnectOptions = url.parse().unwrap();
        let wrong = PgPoolConfig {
            host: opts.get_host().to_owned(),
            port: opts.get_port(),
            database: opts.get_database().unwrap_or("postgres").to_owned(),
            app_role: "daiya_app_login".into(),
            password: zeroize::Zeroizing::new("definitely-wrong".into()),
            max_connections: 1,
        };
        let err = wrong.connect().await.unwrap_err();
        assert!(!err.to_string().contains("definitely-wrong"));
    }
}

#[tokio::test]
async fn row_queries_refuse_what_the_set_lacks_and_compare_times_only_with_times() {
    use daiya_model::{ReadScope, RowQuery, RowSet};
    use odata_table::Cond;
    use odata_url::CompareOp;
    let _schema = SCHEMA_LOCK.read().await;
    let Some(db) = pg().await else { return };
    let store = &db.app;
    let org = OrgId::generate();
    store.upsert_folder(folder(org, "f", None)).await.unwrap();
    for set in RowSet::ALL {
        let refused = |query: RowQuery| async move {
            store.read_rows(org, query).await.unwrap_err().to_string()
        };
        let mut unknown = RowQuery::every(set, ReadScope::All);
        unknown.window.filter = Cond::IsNull {
            path: vec!["nope".into()],
        };
        assert!(
            refused(unknown)
                .await
                .contains(&format!("{} has no property nope", set.name())),
            "{set:?}"
        );
        let mut nested = RowQuery::every(set, ReadScope::All);
        nested.window.filter = Cond::IsNull {
            path: vec![set.key().to_owned(), "x".into()],
        };
        assert!(refused(nested).await.contains("has no property"), "{set:?}");
        let mut cursor = RowQuery::every(set, ReadScope::All);
        cursor.window.after = Some(Vec::new());
        assert!(
            refused(cursor)
                .await
                .contains("the cursor is not one of this order"),
            "{set:?}"
        );
    }
    let set = RowSet::Folders;
    let all = store
        .read_rows(org, RowQuery::every(set, ReadScope::All))
        .await
        .unwrap()
        .rows;
    assert_eq!(all.len(), 1);
    let time = all[0]
        .as_object()
        .unwrap()
        .keys()
        .find(|k| k.ends_with("At"))
        .expect("a time member")
        .clone();
    let mut query = RowQuery::every(set, ReadScope::All);
    query.window.filter = Cond::Or(
        Box::new(Cond::Const(false)),
        Box::new(Cond::Compare {
            path: vec![time.clone()],
            op: CompareOp::Ne,
            value: json!("not a time"),
        }),
    );
    let page = store.read_rows(org, query).await.unwrap();
    assert!(
        page.rows.is_empty(),
        "a time compared with text matches nothing"
    );
    let mut query = RowQuery::every(set, ReadScope::All);
    query.window.filter = Cond::Or(
        Box::new(Cond::Const(false)),
        Box::new(Cond::Not(Box::new(Cond::IsNull { path: vec![time] }))),
    );
    assert_eq!(store.read_rows(org, query).await.unwrap().rows.len(), 1);
    // A NUL reaches no row (the API refuses it first): the database's refusal is a 500.
    let mut query = RowQuery::every(set, ReadScope::All);
    query.window.filter = Cond::Compare {
        path: vec![set.key().to_owned()],
        op: CompareOp::Eq,
        value: json!("a\u{0}b"),
    };
    let err = store.read_rows(org, query).await.unwrap_err();
    assert_eq!(err.status_code(), 500, "{err}");
    assert!(err.to_string().contains("index rows"), "{err}");
}

#[tokio::test]
async fn a_lost_database_is_an_internal_error_that_leaks_nothing() {
    let _schema = SCHEMA_LOCK.read().await;
    let Some(db) = pg().await else { return };
    let store = PgStore::new(db.app_config.connect().await.unwrap());
    store.pool().close().await;
    let org = OrgId::generate();

    let boot = store.verify_rls_enforced().await.unwrap_err();
    assert_eq!(boot.slug(), "cr.rls_not_enforced");
    let mut errors = vec![
        store.get_quotas(org).await.unwrap_err(),
        store
            .set_quotas(InstallationId::SINGLE, org, INSTANCE, OrgQuotas::default())
            .await
            .unwrap_err(),
        store.list_folders(org).await.unwrap_err(),
        store.get_folder(org, "f").await.unwrap_err(),
        store
            .upsert_folder(folder(org, "f", None))
            .await
            .unwrap_err(),
        store.delete_folder(org, "f").await.unwrap_err(),
        store.list_dashboards(org).await.unwrap_err(),
        store.get_dashboard(org, "d").await.unwrap_err(),
        store
            .upsert_dashboard(dashboard(org, "d", None), "", true)
            .await
            .unwrap_err(),
        store.delete_dashboard(org, "d").await.unwrap_err(),
        store.list_dashboard_versions(org, "d").await.unwrap_err(),
        store.list_datasources(org).await.unwrap_err(),
        store.get_datasource(org, "d").await.unwrap_err(),
        store.get_datasource_by_name(org, "d").await.unwrap_err(),
        store
            .upsert_datasource(datasource(org, "d", "D", "prometheus"))
            .await
            .unwrap_err(),
        store.delete_datasource(org, "d").await.unwrap_err(),
        store.list_library_panels(org).await.unwrap_err(),
        store.get_library_panel(org, "l").await.unwrap_err(),
        store.delete_library_panel(org, "l").await.unwrap_err(),
        store.list_public_dashboards(org).await.unwrap_err(),
        store.get_public_dashboard(org, "p").await.unwrap_err(),
        store
            .get_public_dashboard_by_dashboard_uid(org, "d")
            .await
            .unwrap_err(),
        store
            .get_public_dashboard_by_token_hash("abc")
            .await
            .unwrap_err(),
        store
            .upsert_public_dashboard(public_dashboard(org, "p", "d", "t"))
            .await
            .unwrap_err(),
        store.delete_public_dashboard(org, "p").await.unwrap_err(),
        store.list_import_reports(org).await.unwrap_err(),
        store
            .read_rows(
                org,
                daiya_model::RowQuery::every(
                    daiya_model::RowSet::ALL[0],
                    daiya_model::ReadScope::All,
                ),
            )
            .await
            .unwrap_err(),
    ];
    let now = TimestampTz::now();
    errors.push(
        store
            .upsert_library_panel(LibraryPanelRecord {
                installation_id: INST,
                org_id: org,
                instance_id: INSTANCE,
                folder_uid: None,
                uid: "l".into(),
                name: "l".into(),
                panel_type: "stat".into(),
                description: String::new(),
                model: json!({}),
                version: 1,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap_err(),
    );
    errors.push(
        store
            .insert_import_report(
                InstallationId::SINGLE,
                ImportReportRecord {
                    id: "r".into(),
                    org_id: org,
                    instance_id: INSTANCE,
                    dry_run: false,
                    status: "ok".into(),
                    dashboard_count: 0,
                    refused_count: 0,
                    mapped_count: 0,
                    summary: json!({}),
                    created_at: now,
                },
            )
            .await
            .unwrap_err(),
    );
    for err in errors {
        assert_eq!(err.status_code(), 500, "{err}");
        assert_eq!(err.public_detail(), "an internal server error occurred");
    }
    assert!(daiya_store_pg::migrate(store.pool()).await.is_err());
}

#[test]
fn the_rls_catalog_check_refuses_every_unsafe_shape() {
    use daiya_store_pg::{TableRlsStatus, validate_rls_catalog_rows};
    let safe = |name: &str| TableRlsStatus {
        table_name: name.into(),
        rls_enabled: true,
        rls_forced: true,
        policy_count: 1,
    };
    let required: Vec<TableRlsStatus> = daiya_store_pg::REQUIRED_RLS_TABLES
        .iter()
        .map(|t| safe(t))
        .collect();
    validate_rls_catalog_rows(false, false, &required).unwrap();

    let refuse = |bypass: bool, sup: bool, tables: &[TableRlsStatus], needle: &str| {
        let err = validate_rls_catalog_rows(bypass, sup, tables).unwrap_err();
        assert_eq!(err.slug(), "cr.rls_not_enforced");
        assert!(err.to_string().contains(needle), "{needle}: {err}");
    };
    refuse(true, false, &required, "BYPASSRLS");
    refuse(false, true, &required, "SUPERUSER");
    refuse(false, false, &required[1..], "is missing from schema");

    let mut unforced = required.clone();
    unforced[0].rls_forced = false;
    refuse(
        false,
        false,
        &unforced,
        "does not have FORCE ROW LEVEL SECURITY",
    );
    let mut disabled = required.clone();
    disabled[0].rls_enabled = false;
    refuse(
        false,
        false,
        &disabled,
        "does not have FORCE ROW LEVEL SECURITY",
    );
    let mut no_policy = required.clone();
    no_policy[0].policy_count = 0;
    refuse(false, false, &no_policy, "has no RLS policy attached");

    // An extra cr_* table is held to the same rule as the required ones.
    let mut extra = required.clone();
    extra.push(TableRlsStatus {
        policy_count: 0,
        ..safe("cr_extra")
    });
    refuse(false, false, &extra, "cr_extra");
}

#[tokio::test]
async fn migrations_run_for_the_owner_and_fail_cleanly_for_the_app_role() {
    let _schema = SCHEMA_LOCK.write().await;
    let Some(db) = pg().await else { return };
    // The owner's store re-runs nothing when the recorded checksum matches.
    PgStore::new(db.admin.clone())
        .run_migrations()
        .await
        .unwrap();
    // The application role owns nothing and may not migrate.
    let err = db.app.run_migrations().await.unwrap_err();
    assert_eq!(err.slug(), "cr.migration_failed", "{err}");
}

#[test]
fn a_url_the_driver_cannot_use_is_refused() {
    let err = parse_url_without_password("postgres://role@db/dash?sslmode=sometimes").unwrap_err();
    assert_eq!(err.status_code(), 400);
    assert!(
        err.to_string().contains("not a valid postgres URL"),
        "{err}"
    );
}

#[tokio::test]
async fn the_test_database_helper_reports_a_bad_or_unreachable_server() {
    let err = connect_test_database("not a url").await.err().unwrap();
    assert_eq!(err.status_code(), 500);
    let err = connect_test_database("postgres://daiya_nobody@127.0.0.1:1/daiya_none")
        .await
        .err()
        .unwrap();
    assert!(err.to_string().contains("failed to connect"), "{err}");
}
