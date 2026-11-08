#![forbid(unsafe_code)]
#![allow(clippy::disallowed_methods)] // Tests read their database URL; jc runs none of this.
//! A restart keeps state: a new `PgStore` on the same database serves the instance and the
//! repositories an earlier one wrote, once `hydrate_tenant` loaded them under the tenant's RLS
//! context, and loads nothing of another tenant.
//!
//! `SHAKO_TEST_DATABASE_URL` names an admin (superuser) connection; the test creates a fresh
//! database, migrates it as the admin and connects the store as a NOSUPERUSER NOBYPASSRLS login
//! role in `shako_app`. Unset: prints `SKIP` and returns.

use shako_core::{InstanceId, InstanceRecord, OrgId, RepoId, RepoRecord, Store};
use shako_store_pg::PgStore;
use sqlx::AssertSqlSafe;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::str::FromStr;

const APP_LOGIN: &str = "shako_restart_login";

fn repo(instance_id: InstanceId, org_id: OrgId, name: &str) -> RepoRecord {
    let now = chrono::Utc::now();
    RepoRecord {
        id: RepoId::new(),
        instance_id,
        org_id,
        project_id: "doprava".into(),
        owner: "jc".into(),
        name: name.into(),
        description: "config".into(),
        default_branch: "main".into(),
        private: true,
        archived: false,
        size_bytes: 1234,
        next_pr_number: 1,
        created_at: now,
        updated_at: now,
    }
}

#[tokio::test]
async fn a_restarted_store_serves_what_the_last_one_wrote() {
    let Ok(admin_url) = std::env::var("SHAKO_TEST_DATABASE_URL") else {
        println!("SKIP: SHAKO_TEST_DATABASE_URL is unset");
        return;
    };
    let admin_opts = PgConnectOptions::from_str(&admin_url).expect("valid SHAKO_TEST_DATABASE_URL");
    let server = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(admin_opts.clone())
        .await
        .expect("SHAKO_TEST_DATABASE_URL is set but unreachable");
    let db = format!("shako_restart_{}", uuid::Uuid::new_v4().simple());
    sqlx::raw_sql(AssertSqlSafe(format!("CREATE DATABASE {db}")))
        .execute(&server)
        .await
        .expect("create test database");

    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(admin_opts.clone().database(&db))
        .await
        .expect("connect test database");
    PgStore::run_migrations(&admin).await.expect("migrate");
    let pw = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    sqlx::raw_sql(AssertSqlSafe(format!(
        "DO $$ BEGIN CREATE ROLE {APP_LOGIN}; EXCEPTION WHEN duplicate_object THEN NULL; END $$;
         ALTER ROLE {APP_LOGIN} LOGIN INHERIT NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE PASSWORD '{pw}';
         GRANT shako_app TO {APP_LOGIN};"
    )))
    .execute(&admin)
    .await
    .expect("create request login role");
    let app_opts = admin_opts.database(&db).username(APP_LOGIN).password(&pw);

    let instance_id = InstanceId::from_u128(1);
    let org_id = OrgId::from_u128(1);
    let other_org = OrgId::from_u128(2);

    let first = PgStore::connect_with(app_opts.clone(), 2)
        .await
        .expect("first start");
    first
        .upsert_instance(InstanceRecord::default_for(
            instance_id,
            org_id,
            "git.jc.example",
        ))
        .await
        .expect("write instance");
    let written = first
        .create_repo(repo(instance_id, org_id, "config"))
        .await
        .expect("write repo");
    first
        .create_repo(repo(instance_id, other_org, "elsewhere"))
        .await
        .expect("write another tenant's repo");
    first.pool().close().await;
    drop(first);

    let second = PgStore::connect_with(app_opts, 2)
        .await
        .expect("second start");
    let loaded = second
        .hydrate_tenant(instance_id, org_id)
        .await
        .expect("hydrate");
    assert_eq!(
        loaded, 1,
        "one repository of this tenant, none of the other"
    );

    let instance = second
        .get_instance(instance_id, org_id)
        .await
        .unwrap()
        .expect("the instance survives the restart");
    assert_eq!(instance.host, "git.jc.example");
    let read = second
        .get_repo_by_name(instance_id, org_id, "jc", "config")
        .await
        .unwrap()
        .expect("the repository survives the restart");
    assert_eq!(
        (read.id, read.project_id.as_str(), read.size_bytes),
        (written.id, "doprava", 1234)
    );
    assert_eq!(
        second.list_repos(instance_id, org_id).await.unwrap().len(),
        1
    );
    assert!(
        second
            .list_repos(instance_id, other_org)
            .await
            .unwrap()
            .is_empty(),
        "a tenant nobody hydrated stays empty"
    );
    let quota = second.get_quota_usage(instance_id, org_id).await.unwrap();
    assert_eq!((quota.repo_count, quota.git_bytes), (1, 1234));

    // A second hydrate of the same tenant changes nothing.
    assert_eq!(second.hydrate_tenant(instance_id, org_id).await.unwrap(), 1);
    let quota = second.get_quota_usage(instance_id, org_id).await.unwrap();
    assert_eq!((quota.repo_count, quota.git_bytes), (1, 1234));

    second.pool().close().await;
    admin.close().await;
    sqlx::raw_sql(AssertSqlSafe(format!("DROP DATABASE {db} WITH (FORCE)")))
        .execute(&server)
        .await
        .expect("drop test database");
}
