//! The project id registry's corners (project_ids.rs): a registered project's owner, a project the
//! mirror places nowhere, the branch table and its sweep, and the rows of a release before the
//! registry. Branch tests run on a database of their own: the sweep and the table are global.

mod common;

use std::time::Duration;

use jc_types::ProjectId;
use joinedcontext_portal::project_ids::{self, BranchState, NoBranch};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;

use common::unique::unique;

fn url() -> String {
    std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .expect("set JC_PORTAL_TEST_DATABASE_URL to a PostgreSQL database the Portal migrates")
}

async fn pool() -> sqlx::PgPool {
    joinedcontext_portal::db::connect(&url())
        .await
        .expect("connect + migrate")
}

fn dead_pool() -> sqlx::PgPool {
    PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(50))
        .connect_lazy("postgres://nobody@127.0.0.1:1/none")
        .expect("a lazy pool")
}

/// A fresh database of its own, migrated by the Portal's own start.
async fn private_database(test: &str) -> sqlx::PgPool {
    let name = format!("{test}_{}", ProjectId::mint().path_segment());
    let admin = sqlx::PgPool::connect(&url())
        .await
        .expect("the test database");
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .execute(&admin)
        .await
        .expect("a fresh database");
    admin.close().await;
    let mut own = url::Url::parse(&url()).expect("url");
    own.set_path(&format!("/{name}"));
    joinedcontext_portal::db::connect(own.as_str())
        .await
        .expect("connect + migrate")
}

async fn organization(pool: &sqlx::PgPool, name: &str) {
    sqlx::query(
        "INSERT INTO organizations (name, host, state, created_by) \
         VALUES ($1, $1 || '.example', 'active', 'test')",
    )
    .bind(name)
    .execute(pool)
    .await
    .expect("an Organization");
}

/// The branch table is a process-wide static: the tests that read or write it take turns.
static BRANCHES: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn no_names_ask_the_registry_nothing() {
    let minted = project_ids::ensure(&dead_pool(), &[])
        .await
        .expect("no names");
    assert!(minted.is_empty());
}

#[tokio::test]
async fn a_registered_project_names_its_organization_and_key() {
    let pool = pool().await;
    let org = format!("cov-org-{}", unique());
    let project = format!("cov-proj-{}", unique());
    organization(&pool, &org).await;
    let key = project_ids::key(&pool, &org, &project)
        .await
        .expect("key")
        .expect("a key");
    let owned = project_ids::owned(&pool, &project).await.expect("owned");
    assert_eq!(owned, Some((org.clone(), key)));
    let unknown = format!("cov-none-{}", unique());
    assert_eq!(
        project_ids::owned(&pool, &unknown).await.expect("owned"),
        None
    );
    assert!(project_ids::owned(&dead_pool(), &project).await.is_err());
}

#[tokio::test]
async fn a_reserved_project_is_not_a_project_that_anyone_owns() {
    let pool = pool().await;
    let org = format!("cov-org-{}", unique());
    organization(&pool, &org).await;
    assert!(project_ids::reserved(&pool, &org)
        .await
        .expect("reserved")
        .is_some());
    assert_eq!(
        project_ids::owned(&pool, &org).await.expect("owned"),
        None,
        "the reserved project is the Organization's, spelled like it"
    );
}

#[tokio::test]
async fn a_project_the_mirror_places_in_no_organization_has_no_key() {
    let mirror = joinedcontext_portal::store::Mirror::new();
    let project = format!("cov-orphan-{}", unique());
    mirror.upsert(common::envelope(
        "Project",
        &project,
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({}),
    ));
    let key = project_ids::key_of(&dead_pool(), &mirror, &project)
        .await
        .expect("no database is asked");
    assert_eq!(key, None);
    let unknown = project_ids::key_of(&dead_pool(), &mirror, "cov-not-in-the-mirror")
        .await
        .expect("no database is asked");
    assert_eq!(unknown, None);
}

#[tokio::test]
async fn a_project_named_like_the_organizations_namespace_is_left_to_the_reserved_key() {
    let pool = pool().await;
    let project = format!("cov-keyed-{}", unique());
    let mirror = common::keyed_mirror(&pool, &[project.as_str()]).await;
    mirror.upsert(common::envelope(
        "Project",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({}),
    ));
    let keyed = project_ids::keyed_projects(&pool, &mirror)
        .await
        .expect("keyed projects");
    assert_eq!(keyed.iter().filter(|(name, _)| *name == project).count(), 1);
    let own = keyed
        .iter()
        .filter(|(name, _)| name == joinedcontext_portal::permissions::ORG_NAMESPACE)
        .count();
    assert!(own <= 1, "the namespace is listed once at most: {keyed:?}");
}

#[tokio::test]
async fn a_branch_renewed_after_the_table_forgot_it_still_renews_in_the_registry() {
    let _turn = BRANCHES.lock().await;
    let pool = private_database("cov_renew").await;
    let org = format!("cov-org-{}", unique());
    organization(&pool, &org).await;
    let parent = project_ids::key(&pool, &org, "parent")
        .await
        .expect("key")
        .expect("a key");
    let branch = project_ids::open_branch(&pool, parent, "ana", 7, 5)
        .await
        .expect("open")
        .expect("a branch");
    assert!(matches!(
        project_ids::branch_state(&branch.name),
        BranchState::Live(_)
    ));
    sqlx::query("UPDATE project_branches SET swept_at = now() WHERE id = $1::uuid")
        .bind(branch.id.to_string())
        .execute(&pool)
        .await
        .expect("marked swept");
    project_ids::read_branches(&pool).await.expect("read");
    assert_eq!(project_ids::branch_state(&branch.name), BranchState::Gone);
    let renewed = project_ids::renew_branch(&pool, parent, branch.id, "ana")
        .await
        .expect("renew")
        .expect("renewed");
    assert!(renewed > branch.expires_at);
    assert_eq!(
        project_ids::branch_state(&branch.name),
        BranchState::Gone,
        "a branch the table lacks is not put back by a renewal"
    );
}

#[tokio::test]
async fn a_parent_that_is_no_project_or_is_full_opens_no_branch() {
    let _turn = BRANCHES.lock().await;
    let pool = private_database("cov_open").await;
    let org = format!("cov-org-{}", unique());
    organization(&pool, &org).await;
    let stranger = jc_types::ProjectKey::new(
        project_ids::reserved(&pool, &org)
            .await
            .expect("reserved")
            .expect("reserved key")
            .org,
        ProjectId::mint(),
    );
    assert_eq!(
        project_ids::open_branch(&pool, stranger, "ana", 7, 5)
            .await
            .expect("open"),
        Err(NoBranch::NotAParent)
    );
    let parent = project_ids::key(&pool, &org, "parent")
        .await
        .expect("key")
        .expect("a key");
    let first = project_ids::open_branch(&pool, parent, "ana", 7, 1)
        .await
        .expect("open")
        .expect("a branch");
    assert_eq!(
        project_ids::open_branch(&pool, parent, "ana", 7, 1)
            .await
            .expect("open"),
        Err(NoBranch::AtLimit)
    );
    let live = project_ids::live_branch_of(&pool, parent, first.id)
        .await
        .expect("live");
    assert_eq!(live.map(|row| row.name), Some(first.name.clone()));
    let other_parent = project_ids::key(&pool, &org, "other-parent")
        .await
        .expect("key")
        .expect("a key");
    assert_eq!(
        project_ids::live_branch_of(&pool, other_parent, first.id)
            .await
            .expect("live"),
        None,
        "another project's branch is not this one's"
    );
    assert_eq!(
        project_ids::renew_branch(&pool, other_parent, first.id, "ana")
            .await
            .expect("renew"),
        None
    );
}

/// An expired branch of `parent`, as a start of last week left it: expired more than an hour ago.
async fn expired_branch(pool: &sqlx::PgPool, org: &str, parent: jc_types::ProjectKey) -> String {
    let id = ProjectId::mint();
    let name = format!("b-{}", id.path_segment());
    sqlx::query(
        "INSERT INTO project_ids (id, name, parent, organization) VALUES ($1::uuid, $2, $3::uuid, $4)",
    )
    .bind(id.to_string())
    .bind(&name)
    .bind(parent.project.to_string())
    .bind(org)
    .execute(pool)
    .await
    .expect("a branch's project row");
    sqlx::query(
        "INSERT INTO project_branches (id, created_by, created_at, expires_at) \
         VALUES ($1::uuid, 'ana', now() - interval '8 days', now() - interval '1 day')",
    )
    .bind(id.to_string())
    .execute(pool)
    .await
    .expect("an expired branch");
    name
}

#[tokio::test]
async fn an_expired_branch_is_swept_once_and_stays_a_tombstone() {
    let _turn = BRANCHES.lock().await;
    let pool = private_database("cov_sweep").await;
    let org = format!("cov-org-{}", unique());
    organization(&pool, &org).await;
    let parent = project_ids::key(&pool, &org, "parent")
        .await
        .expect("key")
        .expect("a key");
    let expired = expired_branch(&pool, &org, parent).await;
    let live = project_ids::open_branch(&pool, parent, "ana", 7, 5)
        .await
        .expect("open")
        .expect("a branch");
    let swept = project_ids::sweep_expired_branches(&pool)
        .await
        .expect("sweep");
    assert_eq!(swept, vec![expired.clone()], "the live branch is not due");
    assert!(project_ids::sweep_expired_branches(&pool)
        .await
        .expect("sweep")
        .is_empty());
    let listed = project_ids::every_branch_owned(&pool)
        .await
        .expect("branches");
    assert_eq!(
        listed.len(),
        1,
        "the swept branch left the list: {listed:?}"
    );
    assert_eq!(listed[0].1, live.name);
}

#[tokio::test]
async fn a_branch_whose_rows_cannot_be_deleted_waits_for_the_next_pass() {
    let _turn = BRANCHES.lock().await;
    let pool = private_database("cov_refused").await;
    let org = format!("cov-org-{}", unique());
    organization(&pool, &org).await;
    let parent = project_ids::key(&pool, &org, "parent")
        .await
        .expect("key")
        .expect("a key");
    let expired = expired_branch(&pool, &org, parent).await;
    sqlx::query(
        "CREATE FUNCTION cov_refuse() RETURNS trigger LANGUAGE plpgsql AS \
         $$ BEGIN RAISE EXCEPTION 'refused on purpose'; END $$",
    )
    .execute(&pool)
    .await
    .expect("a function");
    sqlx::query(
        "CREATE TRIGGER cov_refuse BEFORE DELETE ON activity \
         FOR EACH STATEMENT EXECUTE FUNCTION cov_refuse()",
    )
    .execute(&pool)
    .await
    .expect("a trigger");
    let first = project_ids::sweep_expired_branches(&pool)
        .await
        .expect("sweep");
    assert!(
        first.is_empty(),
        "a refused delete sweeps nothing: {first:?}"
    );
    sqlx::query("DROP TRIGGER cov_refuse ON activity")
        .execute(&pool)
        .await
        .expect("trigger dropped");
    let second = project_ids::sweep_expired_branches(&pool)
        .await
        .expect("sweep");
    assert_eq!(second, vec![expired]);
}

#[tokio::test(start_paused = true)]
async fn the_branch_watch_warns_and_goes_on_when_the_database_is_down() {
    let _turn = BRANCHES.lock().await;
    project_ids::watch_branches(dead_pool()).await;
    tokio::time::sleep(project_ids::BRANCH_WATCH + Duration::from_secs(1)).await;
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(Duration::from_secs(5)).await;
}

/// A database migrated to the release before the forcing migration 0080 (T-7264).
async fn migrated_before_0080(test: &str) -> sqlx::PgPool {
    let name = format!("{test}_{}", ProjectId::mint().path_segment());
    let admin = sqlx::PgPool::connect(&url())
        .await
        .expect("the test database");
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .execute(&admin)
        .await
        .expect("a fresh database");
    admin.close().await;
    let mut own = url::Url::parse(&url()).expect("url");
    own.set_path(&format!("/{name}"));
    let pool = sqlx::PgPool::connect(own.as_str()).await.expect("fresh");
    let mut migrator = sqlx::migrate!("./migrations");
    migrator.migrations = migrator
        .migrations
        .iter()
        .filter(|m| m.version < 80)
        .cloned()
        .collect::<Vec<_>>()
        .into();
    migrator.run(&pool).await.expect("migrated");
    pool
}

#[tokio::test]
async fn a_row_of_a_branch_is_not_keyed_under_the_installation() {
    let pool = migrated_before_0080("cov_install").await;
    let branch = format!("b-{}", ProjectId::mint().path_segment());
    for project in ["bratislava", "org", branch.as_str()] {
        sqlx::query(
            "INSERT INTO activity (time, project, kind, source, summary, severity) \
             VALUES (now(), $1, 'config.applied', 'portal', 'Applied.', 'info')",
        )
        .bind(project)
        .execute(&pool)
        .await
        .expect("a row of the release before 0080");
    }
    let smartcity = joinedcontext_portal::host_registry::SeedOrganization {
        name: "smartcity".to_owned(),
        organization_id: None,
        host: "portal.smartcity.example".to_owned(),
        catalogue_id: None,
    };
    let keyed = project_ids::key_under_installation(&pool, &smartcity)
        .await
        .expect("keyed");
    assert_eq!(keyed, 2, "the project's row and the Organization's own");
    let unkeyed: Vec<String> = sqlx::query_scalar(
        "SELECT project FROM activity WHERE org_id IS NULL OR project_id IS NULL",
    )
    .fetch_all(&pool)
    .await
    .expect("unkeyed rows");
    assert_eq!(
        unkeyed,
        vec![branch],
        "a branch's row is left for 0080 to set aside"
    );
}
