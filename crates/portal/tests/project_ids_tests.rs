//! The project id registry (ARCH/TEN-01, TEN-04, TEN-12; T-7190, migration 0066): every project
//! the project list shows carries an id minted once, stable across calls, different for two
//! projects, never renamed, released or reused, and a branch with its parent's name never
//! answers for the parent.
//!
//! The store is the database `JC_PORTAL_TEST_DATABASE_URL` names (CI's portal-test jobs set it).
//! A missing variable fails, never skips. Each test uses names of its own run.

mod common;

use axum::http::StatusCode;
use jc_types::ProjectId;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::project_ids;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};

use common::{bind, envelope, person, send};

async fn pool() -> sqlx::PgPool {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .expect("set JC_PORTAL_TEST_DATABASE_URL to a PostgreSQL database the Portal migrates");
    joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate")
}

fn run_name(test: &str) -> String {
    format!("{test}-{}", ProjectId::mint().path_segment())
}

#[tokio::test]
async fn ids_are_minted_once_and_differ_per_project() {
    let pool = pool().await;
    let a = run_name("ids-a");
    let b = run_name("ids-b");
    let first = project_ids::ensure(&pool, &[a.clone(), b.clone()])
        .await
        .expect("ensure");
    let again = project_ids::ensure(&pool, &[b.clone(), a.clone()])
        .await
        .expect("ensure again");
    assert_eq!(first.len(), 2);
    assert_eq!(first, again, "an id is minted once and read back after");
    assert_ne!(first[&a], first[&b], "two projects never share an id");
}

#[tokio::test]
async fn an_id_is_never_renamed_released_or_reused() {
    let pool = pool().await;
    let name = run_name("keep");
    let id = project_ids::ensure(&pool, std::slice::from_ref(&name))
        .await
        .expect("ensure")[&name];
    for statement in [
        "DELETE FROM project_ids WHERE id = $1::uuid",
        "UPDATE project_ids SET name = name || '-renamed' WHERE id = $1::uuid",
        "UPDATE project_ids SET id = gen_random_uuid() WHERE id = $1::uuid",
    ] {
        let refused = sqlx::query(statement)
            .bind(id.to_string())
            .execute(&pool)
            .await
            .expect_err(statement);
        assert!(
            refused
                .to_string()
                .contains("never renamed, released or reused"),
            "{statement}: {refused}"
        );
    }
    let still = project_ids::ensure(&pool, std::slice::from_ref(&name))
        .await
        .expect("ensure");
    assert_eq!(still[&name], id);
}

#[tokio::test]
async fn a_branch_with_its_parents_name_never_answers_for_the_parent() {
    let pool = pool().await;
    let name = run_name("branch");
    let parent = project_ids::ensure(&pool, std::slice::from_ref(&name))
        .await
        .expect("ensure")[&name];
    let branch = ProjectId::mint();
    sqlx::query("INSERT INTO project_ids (id, name, parent) VALUES ($1::uuid, $2, $3::uuid)")
        .bind(branch.to_string())
        .bind(&name)
        .bind(parent.to_string())
        .execute(&pool)
        .await
        .expect("a branch may hold its parent's name");
    let live = project_ids::ensure(&pool, std::slice::from_ref(&name))
        .await
        .expect("ensure");
    assert_eq!(live[&name], parent);
    assert_ne!(live[&name], branch);
}

#[tokio::test]
async fn the_project_list_shows_each_readable_projects_id() {
    let pool = pool().await;
    let state = AppState::new(Config::for_tests(), None).with_db(pool.clone());
    let project = run_name("listed");
    let hidden = run_name("hidden");
    for name in [&project, &hidden] {
        state.mirror.upsert(envelope(
            "Project",
            name,
            joinedcontext_portal::permissions::ORG_NAMESPACE,
            json!({}),
        ));
    }
    bind(&state.mirror, "ada", &project);
    let answer = send(&state, person("ada"), "GET", "/api/v1/projects", None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let body: Value = serde_json::from_str(&answer.text).expect("json");
    let items = body["items"].as_array().cloned().unwrap_or_default();
    let listed: Vec<&Value> = items
        .iter()
        .filter(|item| item["name"] == project.as_str())
        .collect();
    assert_eq!(listed.len(), 1, "{body}");
    let id = listed[0]["id"].as_str().expect("an id");
    let expected = project_ids::ensure(&pool, std::slice::from_ref(&project))
        .await
        .expect("ensure")[&project];
    assert_eq!(ProjectId::from_path_segment(id), Ok(expected));
    assert!(
        items.iter().all(|item| item["name"] != hidden.as_str()),
        "a project the caller cannot read is not listed: {body}"
    );
}

/// An active Organization of the registry named for this run, as the host registry writes one.
async fn organization(pool: &sqlx::PgPool, test: &str) -> String {
    let name = run_name(test);
    sqlx::query(
        "INSERT INTO organizations (name, host, state, created_by) VALUES ($1, $1 || '.example', 'active', 'test')",
    )
    .bind(&name)
    .execute(pool)
    .await
    .expect("an Organization");
    name
}

#[tokio::test]
async fn every_organization_gets_a_key_and_one_reserved_project() {
    let pool = pool().await;
    let org = organization(&pool, "keyorg").await;
    let reserved = project_ids::reserved(&pool, &org)
        .await
        .expect("reserved")
        .expect("an Organization of the registry has a reserved project");
    let again = project_ids::reserved(&pool, &org).await.expect("reserved");
    assert_eq!(Some(reserved), again, "minted once, read back after");
    let other = organization(&pool, "keyother").await;
    let other = project_ids::reserved(&pool, &other)
        .await
        .expect("reserved");
    assert_ne!(
        other.map(|k| k.org),
        Some(reserved.org),
        "two Organizations, two keys"
    );
    let refused = sqlx::query("UPDATE organizations SET key = gen_random_uuid() WHERE name = $1")
        .bind(&org)
        .execute(&pool)
        .await
        .expect_err("an Organization's key never changes");
    assert!(
        refused
            .to_string()
            .contains("keeps its name, its key, its id and its catalogue"),
        "{refused}"
    );
}

#[tokio::test]
async fn a_projects_key_is_its_organizations_and_never_another() {
    let pool = pool().await;
    let org = organization(&pool, "owner").await;
    let foreign = organization(&pool, "foreign").await;
    let project = run_name("keyed");
    let key = project_ids::key(&pool, &org, &project)
        .await
        .expect("key")
        .expect("a project of an active Organization has a key");
    let reserved = project_ids::reserved(&pool, &org).await.expect("reserved");
    assert_eq!(Some(key.org), reserved.map(|k| k.org));
    assert_ne!(
        Some(key.project),
        reserved.map(|k| k.project),
        "the reserved id is never a project's"
    );
    let ids = project_ids::ensure(&pool, std::slice::from_ref(&project))
        .await
        .expect("ensure");
    assert_eq!(
        ids[&project], key.project,
        "the key holds the registry's id"
    );
    assert_eq!(
        project_ids::key(&pool, &foreign, &project)
            .await
            .expect("key"),
        None,
        "a project of one Organization never gets a key in another"
    );
    assert_eq!(
        project_ids::key(&pool, &org, &project).await.expect("key"),
        Some(key)
    );
}

#[tokio::test]
async fn a_project_spelled_like_its_organization_never_gets_the_reserved_id() {
    let pool = pool().await;
    let org = organization(&pool, "same").await;
    let reserved = project_ids::reserved(&pool, &org)
        .await
        .expect("reserved")
        .expect("one");
    let key = project_ids::key(&pool, &org, &org)
        .await
        .expect("key")
        .expect("a key");
    assert_ne!(key.project, reserved.project);
    let ids = project_ids::ensure(&pool, std::slice::from_ref(&org))
        .await
        .expect("ensure");
    assert_ne!(ids[&org], reserved.project);
}

#[tokio::test]
async fn no_key_without_an_active_organization_of_the_registry() {
    let pool = pool().await;
    let project = run_name("orphan");
    assert_eq!(
        project_ids::key(&pool, &run_name("nowhere"), &project)
            .await
            .expect("key"),
        None,
        "there is no default Organization (TEN-05)"
    );
    let gone = organization(&pool, "gone").await;
    sqlx::query("UPDATE organizations SET state = 'removed' WHERE name = $1")
        .bind(&gone)
        .execute(&pool)
        .await
        .expect("removed");
    assert_eq!(
        project_ids::key(&pool, &gone, &project).await.expect("key"),
        None
    );
    assert_eq!(
        project_ids::reserved(&pool, &gone).await.expect("reserved"),
        None
    );
}

#[tokio::test]
async fn migration_0067_keys_the_organizations_and_projects_of_today() {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL").expect("JC_PORTAL_TEST_DATABASE_URL");
    let name = format!("pk_{}", ProjectId::mint().path_segment());
    let admin = sqlx::PgPool::connect(&url)
        .await
        .expect("the test database");
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .execute(&admin)
        .await
        .expect("a fresh database");
    let mut own = url::Url::parse(&url).expect("url");
    own.set_path(&format!("/{name}"));
    let pool = sqlx::PgPool::connect(own.as_str()).await.expect("fresh");
    let mut before = sqlx::migrate!("./migrations");
    before.migrations = before
        .migrations
        .iter()
        .filter(|m| m.version < 67)
        .cloned()
        .collect::<Vec<_>>()
        .into();
    before.run(&pool).await.expect("migrated to 0066");
    sqlx::query(
        "INSERT INTO organizations (name, host, state, created_by) VALUES \
         ('alfa', 'alfa.example', 'active', 't'), ('bravo', 'bravo.example', 'removed', 't')",
    )
    .execute(&pool)
    .await
    .expect("today's Organizations");
    sqlx::query("INSERT INTO project_ids (id, name) VALUES (gen_random_uuid(), 'alfa'), (gen_random_uuid(), 'city')")
        .execute(&pool)
        .await
        .expect("today's projects");
    sqlx::query(
        "INSERT INTO organization_backup_log (organization, kind, action, actor, ok) VALUES \
         ('alfa', 'data', 'run', 't', true), ('zulu', 'data', 'run', 't', true)",
    )
    .execute(&pool)
    .await
    .expect("today's backup audit");
    let today: Vec<(String, String)> =
        sqlx::query_as("SELECT name, id::text FROM project_ids ORDER BY name")
            .fetch_all(&pool)
            .await
            .expect("today's ids");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("0067");
    let keys: Vec<String> = sqlx::query_scalar("SELECT key::text FROM organizations ORDER BY name")
        .fetch_all(&pool)
        .await
        .expect("keys");
    assert_eq!(keys.len(), 2);
    assert_ne!(keys[0], keys[1], "each Organization its own key");
    let reserved: i64 = sqlx::query_scalar("SELECT count(*) FROM project_ids WHERE reserved")
        .fetch_one(&pool)
        .await
        .expect("reserved");
    assert_eq!(
        reserved, 2,
        "one reserved project per Organization, a removed one too"
    );
    let ids = project_ids::ensure(&pool, &["alfa".to_owned(), "city".to_owned()])
        .await
        .expect("ensure");
    for (project, id) in today {
        assert_eq!(ids[&project].to_string(), id, "{project} keeps today's id");
    }
    let alfa = project_ids::reserved(&pool, "alfa")
        .await
        .expect("reserved")
        .expect("a key");
    let audit: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT organization, org_id::text, project_id::text FROM organization_backup_log ORDER BY organization",
    )
    .fetch_all(&pool)
    .await
    .expect("the audit, read as its owner");
    assert_eq!(
        audit,
        vec![
            (
                "alfa".to_owned(),
                Some(alfa.org.to_string()),
                Some(alfa.project.to_string())
            ),
            ("zulu".to_owned(), None, None),
        ],
        "a row of a registry Organization gets its reserved key; one outside keeps none and stays"
    );
    pool.close().await;
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP DATABASE {name}")))
        .execute(&admin)
        .await
        .expect("dropped");
}

const KEYED: [&str; 3] = [
    "organization_backup_targets",
    "organization_backup_log",
    "organization_data_restores",
];

#[tokio::test]
async fn every_keyed_table_has_forced_row_level_security_on_the_key_alone() {
    let pool = pool().await;
    let unforced: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname::text FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         JOIN pg_attribute a ON a.attrelid = c.oid AND a.attname = 'org_id' AND NOT a.attisdropped \
         WHERE n.nspname = 'public' AND c.relkind = 'r' \
         AND NOT (c.relrowsecurity AND c.relforcerowsecurity) ORDER BY 1",
    )
    .fetch_all(&pool)
    .await
    .expect("catalog");
    // Phase one of T-7196 keys these by name first; the forcing migration takes them off the list.
    let unforced: Vec<String> = unforced
        .into_iter()
        .filter(|t| !project_ids::KEYED_BY_NAME.contains(&t.as_str()))
        .collect();
    assert!(
        unforced.is_empty(),
        "a table with org_id and no forced RLS (STO-03): {unforced:?}"
    );
    for table in project_ids::KEYED_BY_NAME {
        let ready: (i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM pg_policies WHERE schemaname = 'public' \
                     AND tablename = $1 AND policyname = 'tenant'), \
                    (SELECT count(*) FROM information_schema.columns WHERE table_schema = 'public' \
                     AND table_name = $1 AND column_name IN ('org_id', 'project_id') \
                     AND column_default LIKE '%current_setting(''jc.%')",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .expect("catalog");
        assert_eq!(
            ready,
            (1, 2),
            "{table}: the tenant policy and both key columns defaulting to the transaction's key"
        );
    }
    let keyed: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname::text FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'public' AND c.relforcerowsecurity ORDER BY 1",
    )
    .fetch_all(&pool)
    .await
    .expect("catalog");
    for table in KEYED {
        assert!(
            keyed.iter().any(|t| t == table),
            "{table} is keyed: {keyed:?}"
        );
    }
    let policies: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT tablename::text, coalesce(qual, ''), coalesce(with_check, '') FROM pg_policies \
         WHERE schemaname = 'public'",
    )
    .fetch_all(&pool)
    .await
    .expect("policies");
    for (table, using, check) in &policies {
        for text in [using, check] {
            let settings: Vec<&str> = text
                .match_indices("current_setting('")
                .map(|(at, m)| text[at + m.len()..].split('\'').next().unwrap_or(""))
                .collect();
            assert!(
                settings.contains(&"jc.org") && settings.contains(&"jc.project"),
                "{table}: the policy keys on jc.org and jc.project: {text}"
            );
            assert!(
                settings
                    .iter()
                    .all(|s| ["jc.org", "jc.project", "jc.space"].contains(s)),
                "{table}: a policy reads only the key (STO-04): {text}"
            );
        }
    }
}

/// A transaction as the role `pk_rls_reader`, which owns nothing and has no BYPASSRLS, as the
/// serving role of T-7179; `key` set as `db::tenant_tx` sets it.
async fn as_reader(
    pool: &sqlx::PgPool,
    key: Option<jc_types::ProjectKey>,
) -> sqlx::Transaction<'static, sqlx::Postgres> {
    sqlx::raw_sql(
        "DO $$ BEGIN CREATE ROLE pk_rls_reader NOLOGIN NOBYPASSRLS; \
         EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$; \
         GRANT SELECT, INSERT, UPDATE, DELETE ON organization_backup_log TO pk_rls_reader; \
         GRANT USAGE ON ALL SEQUENCES IN SCHEMA public TO pk_rls_reader",
    )
    .execute(pool)
    .await
    .expect("the reader role");
    let mut tx = match key {
        Some(key) => joinedcontext_portal::db::tenant_tx(pool, key)
            .await
            .expect("keyed"),
        None => pool.begin().await.expect("begin"),
    };
    sqlx::query("SET LOCAL ROLE pk_rls_reader")
        .execute(&mut *tx)
        .await
        .expect("role");
    tx
}

#[tokio::test]
async fn a_backup_row_answers_only_under_its_organizations_key() {
    let pool = pool().await;
    let org = organization(&pool, "rlsorg").await;
    let other = organization(&pool, "rlsother").await;
    let key = project_ids::reserved(&pool, &org)
        .await
        .expect("reserved")
        .expect("a key");
    let other_key = project_ids::reserved(&pool, &other)
        .await
        .expect("reserved")
        .expect("a key");
    let project = project_ids::key(&pool, &org, &run_name("plain"))
        .await
        .expect("key")
        .expect("a key");
    let mut tx = as_reader(&pool, Some(key)).await;
    sqlx::query(
        "INSERT INTO organization_backup_log (organization, kind, action, actor, ok) \
         VALUES ($1, 'data', 'run', 'test', true)",
    )
    .bind(&org)
    .execute(&mut *tx)
    .await
    .expect("a write under the key is stamped with it");
    tx.commit().await.expect("commit");
    let count = |key: Option<jc_types::ProjectKey>| {
        let pool = pool.clone();
        let org = org.clone();
        async move {
            let mut tx = as_reader(&pool, key).await;
            let n: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM organization_backup_log WHERE organization = $1",
            )
            .bind(&org)
            .fetch_one(&mut *tx)
            .await
            .expect("count");
            n
        }
    };
    assert_eq!(count(Some(key)).await, 1);
    assert_eq!(count(None).await, 0, "the key unset reads no row");
    assert_eq!(
        count(Some(other_key)).await,
        0,
        "another Organization's key reads no row"
    );
    assert_eq!(
        count(Some(project)).await,
        0,
        "a project of the same Organization reads no row"
    );
    let mut tx = as_reader(&pool, Some(other_key)).await;
    let refused = sqlx::query(
        "INSERT INTO organization_backup_log (organization, kind, action, actor, ok, org_id, project_id) \
         VALUES ($1, 'data', 'run', 'test', true, $2::uuid, $3::uuid)",
    )
    .bind(&org)
    .bind(key.org.to_string())
    .bind(key.project.to_string())
    .execute(&mut *tx)
    .await
    .expect_err("a row of another key is never written");
    assert!(
        refused.to_string().contains("row-level security"),
        "{refused}"
    );
    let mut tx = as_reader(&pool, None).await;
    sqlx::query(
        "INSERT INTO organization_backup_log (organization, kind, action, actor, ok) \
         VALUES ($1, 'data', 'run', 'test', true)",
    )
    .bind(&org)
    .execute(&mut *tx)
    .await
    .expect_err("the key unset writes no row");
}

/// A grafana_visualizations row of `project` written as before T-7196: no key.
async fn unkeyed_visualization(pool: &sqlx::PgPool, project: &str) -> String {
    let id = run_name("viz");
    sqlx::query(
        "INSERT INTO grafana_visualizations (id, project, title, endpoint, entity_type, attribute, kind) \
         VALUES ($1, $2, 'Free bikes', 'bikes', 'BikeHireDockingStation', 'availableBikeNumber', 'stat')",
    )
    .bind(&id)
    .bind(project)
    .execute(pool)
    .await
    .expect("a row of today");
    id
}

async fn key_columns(pool: &sqlx::PgPool, id: &str) -> (Option<String>, Option<String>) {
    sqlx::query_as(
        "SELECT org_id::text, project_id::text FROM grafana_visualizations WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .expect("the row")
}

/// T-7196 phase one: a row written before the key is keyed by the backfill once the mirror places
/// its project; a project the mirror lacks, or whose Organization is not of the registry, keeps
/// no key and is named; a row written through the store carries the key by the column default;
/// and the policy the forcing migration turns on lets each key read only its own rows.
#[tokio::test]
async fn project_rows_are_keyed_by_the_backfill_and_the_policy_reads_only_their_key() {
    use joinedcontext_portal::grafana::{self, Kind, Visualization};
    let pool = pool().await;
    let state = AppState::new(Config::for_tests(), None).with_db(pool.clone());
    let org = organization(&pool, "bforg").await;
    let placed = run_name("bfplaced");
    let orphan = run_name("bforphan");
    let gone = run_name("bfgone");
    for (project, organization) in [(&placed, org.clone()), (&orphan, run_name("bfnoorg"))] {
        state.mirror.upsert(envelope(
            "Project",
            project,
            joinedcontext_portal::permissions::ORG_NAMESPACE,
            json!({ "organizationRef": organization }),
        ));
    }
    let old = unkeyed_visualization(&pool, &placed).await;
    let unplaced = unkeyed_visualization(&pool, &orphan).await;
    let removed = unkeyed_visualization(&pool, &gone).await;

    let waiting = project_ids::backfill(&pool, &state.mirror)
        .await
        .expect("backfill");
    let key = project_ids::key_of(&pool, &state.mirror, &placed)
        .await
        .expect("key")
        .expect("a placed project has a key");
    assert_eq!(
        key_columns(&pool, &old).await,
        (Some(key.org.to_string()), Some(key.project.to_string()))
    );
    for (project, id) in [(&orphan, &unplaced), (&gone, &removed)] {
        assert!(waiting.contains(project), "{project} waits: {waiting:?}");
        assert_eq!(key_columns(&pool, id).await, (None, None), "{project}");
    }
    assert!(!waiting.contains(&placed), "{waiting:?}");
    assert_eq!(
        project_ids::key_of(&pool, &state.mirror, &gone)
            .await
            .expect("key"),
        None,
        "a name the mirror holds no Project of gets no key, and no id minted"
    );

    let new = Visualization {
        id: run_name("viznew"),
        project: placed.clone(),
        title: "Free bikes over time".into(),
        endpoint: "bikes".into(),
        entity_type: "BikeHireDockingStation".into(),
        attribute: "availableBikeNumber".into(),
        kind: Kind::Timeseries,
        created_at: String::new(),
    };
    grafana::insert(&pool, key, &new).await.expect("insert");
    assert_eq!(
        key_columns(&pool, &new.id).await,
        (Some(key.org.to_string()), Some(key.project.to_string())),
        "a row written in the tenant transaction carries its key"
    );
    let listed = grafana::list(&pool, key, &placed).await.expect("list");
    assert_eq!(listed.len(), 2, "{listed:?}");

    // What the forcing migration turns on, rolled back here: the key alone decides what is read.
    let other = project_ids::reserved(&pool, &org)
        .await
        .expect("reserved")
        .expect("a key");
    sqlx::raw_sql(
        "DO $$ BEGIN CREATE ROLE pk_rls_reader NOLOGIN NOBYPASSRLS; \
         EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$; \
         GRANT SELECT ON grafana_visualizations TO pk_rls_reader",
    )
    .execute(&pool)
    .await
    .expect("the reader role");
    for (key, expected) in [(Some(key), 2_i64), (Some(other), 0), (None, 0)] {
        let mut tx = pool.begin().await.expect("begin");
        sqlx::raw_sql(
            "ALTER TABLE grafana_visualizations ENABLE ROW LEVEL SECURITY; \
             ALTER TABLE grafana_visualizations FORCE ROW LEVEL SECURITY",
        )
        .execute(&mut *tx)
        .await
        .expect("forced in this transaction");
        if let Some(key) = key {
            sqlx::query(
                "SELECT set_config('jc.org', $1, true), set_config('jc.project', $2, true)",
            )
            .bind(key.org.to_string())
            .bind(key.project.to_string())
            .execute(&mut *tx)
            .await
            .expect("keyed");
        }
        sqlx::query("SET LOCAL ROLE pk_rls_reader")
            .execute(&mut *tx)
            .await
            .expect("role");
        let n: i64 =
            sqlx::query_scalar("SELECT count(*) FROM grafana_visualizations WHERE project = $1")
                .bind(&placed)
                .fetch_one(&mut *tx)
                .await
                .expect("count");
        assert_eq!(n, expected, "{key:?}");
        tx.rollback().await.expect("rollback");
    }
}

/// A row of each table of T-7197 for `project`, written as before the key: what the backfill
/// finds on a database of today. `$1` is the project.
const ALERTS_HEALTH_ROWS: &[(&str, &str)] = &[
    (
        "alert_subscriptions",
        "INSERT INTO alert_subscriptions (subject, project, scope, target, events, delivery) \
         VALUES ('jana', $1, 'pipeline', 'bikes', '{failure}', 'portal')",
    ),
    (
        "alert_incidents",
        "INSERT INTO alert_incidents (project, pipeline, event) VALUES ($1, 'bikes', 'failure')",
    ),
    (
        "alert_notices",
        "INSERT INTO alert_notices (recipient, subscription, project, pipeline, event, change) \
         SELECT 'jana', id, $1, 'bikes', 'failure', 'opened' FROM alert_subscriptions WHERE project = $1",
    ),
    (
        "health_check",
        "INSERT INTO health_check (project, kind, name, \"check\", status, since, checked_at) \
         VALUES ($1, 'pipeline', 'bikes', 'runs', 'ok', now(), now())",
    ),
    (
        "health_check_history",
        "INSERT INTO health_check_history (project, kind, name, \"check\", at, status) \
         VALUES ($1, 'pipeline', 'bikes', 'runs', now(), 'ok')",
    ),
    (
        "space_quality",
        "INSERT INTO space_quality (project, space, report) VALUES ($1, 'mobility', '{}')",
    ),
    (
        "endpoint_usage_daily",
        "INSERT INTO endpoint_usage_daily (project, endpoint, surface, day, caller, calls) \
         VALUES ($1, 'bikes', 'ogc', current_date, 'person', 1)",
    ),
    (
        "link_check",
        "INSERT INTO link_check (project, pipeline, checked_at, result) VALUES ($1, 'bikes', now(), '{}')",
    ),
];

/// T-7197 PK-1e: every alerts, health, quality, usage and link check row of today is keyed by
/// the backfill once the mirror places its project, a project with no Organization waits, and
/// under the policy the forcing migration turns on each key reads only its own rows. The stores
/// write new rows in the key, and keep none for a project without one.
#[tokio::test]
async fn alerts_health_and_quality_rows_are_keyed_and_read_only_under_their_key() {
    use joinedcontext_portal::alerts::{Delivery, Event, Scope};
    let (state, placed, orphan, key) =
        rows_keyed_and_read_only_under_their_key(ALERTS_HEALTH_ROWS, "ah").await;
    let pool = state.db.clone().expect("a database");
    // The stores write in the key, and keep nothing for a project without one.
    let made = state
        .alerts
        .upsert(
            "eeva",
            &placed,
            Scope::Pipeline,
            "trams",
            &[Event::Stale],
            Delivery::Portal,
        )
        .await
        .expect("upsert")
        .expect("a placed project keeps alerts");
    let stamped: (Option<String>,) =
        sqlx::query_as("SELECT project_id::text FROM alert_subscriptions WHERE id = $1")
            .bind(made.id)
            .fetch_one(&pool)
            .await
            .expect("row");
    assert_eq!(stamped.0, Some(key.project.to_string()));
    assert_eq!(
        state
            .alerts
            .upsert(
                "eeva",
                &orphan,
                Scope::Pipeline,
                "trams",
                &[Event::Stale],
                Delivery::Portal
            )
            .await
            .expect("upsert"),
        None,
        "a project with no Organization of the registry keeps no alert"
    );
    let theirs = state.alerts.subscriptions_of("eeva").await.expect("read");
    assert!(
        theirs.iter().any(|s| s.id == made.id) && theirs.iter().all(|s| s.project != orphan),
        "{theirs:?}"
    );
    assert!(state.alerts.remove("eeva", made.id).await.expect("remove"));
}

/// Rows of `rows` (table, insert binding `$1` to the project) for a placed and an unplaced
/// project of this run: the backfill keys the placed one's and names the other as waiting, and
/// under the policy the forcing migration turns on (rolled back here) each table reads the placed
/// project's row under its key only. Answers the state, the two projects and the key.
async fn rows_keyed_and_read_only_under_their_key(
    rows: &'static [(&'static str, &'static str)],
    test: &str,
) -> (AppState, String, String, jc_types::ProjectKey) {
    let pool = pool().await;
    let state = AppState::new(Config::for_tests(), None).with_db(pool.clone());
    let org = organization(&pool, &format!("{test}org")).await;
    let placed = run_name(&format!("{test}placed"));
    let orphan = run_name(&format!("{test}orphan"));
    for (project, organization) in [
        (&placed, org.clone()),
        (&orphan, run_name(&format!("{test}noorg"))),
    ] {
        state.mirror.upsert(envelope(
            "Project",
            project,
            joinedcontext_portal::permissions::ORG_NAMESPACE,
            json!({ "organizationRef": organization }),
        ));
    }
    for project in [&placed, &orphan] {
        for (table, insert) in rows {
            sqlx::query(*insert)
                .bind(project)
                .execute(&pool)
                .await
                .unwrap_or_else(|err| panic!("{table}: {err}"));
        }
    }
    for (table, _) in rows {
        assert!(
            project_ids::KEYED_BY_NAME.contains(table),
            "{table} is backfilled"
        );
    }

    let waiting = project_ids::backfill(&pool, &state.mirror)
        .await
        .expect("backfill");
    assert!(waiting.contains(&orphan), "{waiting:?}");
    assert!(!waiting.contains(&placed), "{waiting:?}");
    let key = project_ids::key_of(&pool, &state.mirror, &placed)
        .await
        .expect("key")
        .expect("a placed project has a key");
    for (table, _) in rows {
        let keyed: Vec<(String, Option<String>, Option<String>)> =
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "SELECT project, org_id::text, project_id::text FROM {table} WHERE project = ANY($1)"
            )))
            .bind([placed.clone(), orphan.clone()])
            .fetch_all(&pool)
            .await
            .expect("rows");
        assert_eq!(keyed.len(), 2, "{table}: {keyed:?}");
        for (project, org_id, project_id) in keyed {
            let expected = if project == placed {
                (Some(key.org.to_string()), Some(key.project.to_string()))
            } else {
                (None, None)
            };
            assert_eq!((org_id, project_id), expected, "{table} {project}");
        }
    }

    // What the forcing migration turns on, rolled back here: the key alone decides what is read.
    let other = project_ids::reserved(&pool, &org)
        .await
        .expect("reserved")
        .expect("a key");
    let tables: Vec<&str> = rows.iter().map(|(t, _)| *t).collect();
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "DO $$ BEGIN CREATE ROLE pk_rls_reader NOLOGIN NOBYPASSRLS; \
         EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$; \
         GRANT SELECT ON {} TO pk_rls_reader",
        tables.join(", ")
    )))
    .execute(&pool)
    .await
    .expect("the reader role");
    for table in &tables {
        for (key, expected) in [(Some(key), 1_i64), (Some(other), 0), (None, 0)] {
            let mut tx = pool.begin().await.expect("begin");
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "ALTER TABLE {table} ENABLE ROW LEVEL SECURITY; \
                 ALTER TABLE {table} FORCE ROW LEVEL SECURITY"
            )))
            .execute(&mut *tx)
            .await
            .expect("forced in this transaction");
            if let Some(key) = key {
                sqlx::query(
                    "SELECT set_config('jc.org', $1, true), set_config('jc.project', $2, true)",
                )
                .bind(key.org.to_string())
                .bind(key.project.to_string())
                .execute(&mut *tx)
                .await
                .expect("keyed");
            }
            sqlx::query("SET LOCAL ROLE pk_rls_reader")
                .execute(&mut *tx)
                .await
                .expect("role");
            let n: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT count(*) FROM {table} WHERE project = $1"
            )))
            .bind(&placed)
            .fetch_one(&mut *tx)
            .await
            .expect("count");
            assert_eq!(n, expected, "{table} {key:?}");
            tx.rollback().await.expect("rollback");
        }
    }
    (state, placed, orphan, key)
}

/// Places `project` under a new Organization of the registry and runs the backfill, as an operator
/// adding the Organization does: the rows it kept unkeyed take its key. Answers the key.
async fn placed_later(state: &AppState, project: &str, test: &str) -> jc_types::ProjectKey {
    let pool = state.db.clone().expect("a database");
    let org = organization(&pool, test).await;
    state.mirror.upsert(envelope(
        "Project",
        project,
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({ "organizationRef": org }),
    ));
    let waiting = project_ids::backfill(&pool, &state.mirror)
        .await
        .expect("backfill");
    assert!(!waiting.iter().any(|w| w == project), "{waiting:?}");
    project_ids::key_of(&pool, &state.mirror, project)
        .await
        .expect("key")
        .expect("a key once placed")
}

/// A draft of `project`, written as before the key (T-7204). `$1` is the project.
const DRAFT_ROWS_0076: &[(&str, &str)] = &[(
    "drafts",
    "INSERT INTO drafts (project, kind, name, manifest, touched_by, touched_kind) \
     VALUES ($1, 'Endpoint', 'air', '{}', 'anna', 'human')",
)];

/// T-7204 PK-1g: the drafts of today are keyed by the backfill and read only under their key; the
/// store writes a new draft in the key, and a project without a key yet (one made before the
/// registry knew its Organization) keeps working unkeyed until the backfill keys it.
#[tokio::test]
async fn drafts_are_keyed_and_read_only_under_their_key() {
    let (state, placed, orphan, key) =
        rows_keyed_and_read_only_under_their_key(DRAFT_ROWS_0076, "dr").await;
    let pool = state.db.clone().expect("a database");
    let manifest = json!({ "kind": "Endpoint", "metadata": { "name": "water" } });
    state
        .drafts
        .put(
            &placed,
            "Endpoint",
            "water",
            manifest.clone(),
            None,
            "anna",
            "human",
        )
        .await
        .expect("a placed project keeps a draft");
    state
        .drafts
        .put(
            &orphan, "Endpoint", "water", manifest, None, "anna", "human",
        )
        .await
        .expect("a project with no key yet keeps its draft unkeyed (phase one)");
    let stamped: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT project, project_id::text FROM drafts WHERE name = 'water' AND project = ANY($1) \
         ORDER BY project = $2",
    )
    .bind(vec![placed.clone(), orphan.clone()])
    .bind(&orphan)
    .fetch_all(&pool)
    .await
    .expect("rows");
    assert_eq!(
        stamped,
        [
            (placed.clone(), Some(key.project.to_string())),
            (orphan.clone(), None)
        ]
    );
    assert_eq!(state.drafts.list(&orphan).await.expect("list").len(), 2);
    assert!(state
        .drafts
        .get(&orphan, "Endpoint", "air")
        .await
        .expect("get")
        .is_some());
    let later = placed_later(&state, &orphan, "drlate").await;
    let keyed: Vec<Option<String>> =
        sqlx::query_scalar("SELECT project_id::text FROM drafts WHERE project = $1")
            .bind(&orphan)
            .fetch_all(&pool)
            .await
            .expect("rows");
    assert_eq!(keyed, vec![Some(later.project.to_string()); 2]);
    let names: Vec<String> = state
        .drafts
        .list(&placed)
        .await
        .expect("list")
        .into_iter()
        .map(|d| d.name)
        .collect();
    assert_eq!(
        names.len(),
        2,
        "the backfilled draft and the new one: {names:?}"
    );
    assert!(state
        .drafts
        .drop(&placed, "Endpoint", "air")
        .await
        .expect("drop"));
}

/// An activity event of `project`, written as before the key (T-7205). `$1` is the project.
const ACTIVITY_ROWS_0077: &[(&str, &str)] = &[(
    "activity",
    "INSERT INTO activity (time, project, kind, source, summary, severity) \
     VALUES (now() - interval '1 minute', $1, 'config.applied', 'reconciler', 'Applied.', 'info')",
)];

/// T-7205 PK-1g: the activity of today is keyed by the backfill and read only under its key; a
/// new event lands in its project's key, and a project without a key yet keeps and reads its
/// events unkeyed until the backfill keys them.
#[tokio::test]
async fn activity_rows_are_keyed_and_read_only_under_their_key() {
    use joinedcontext_portal::activity::{ActivityEvent, ActivityFilter};
    let (state, placed, orphan, key) =
        rows_keyed_and_read_only_under_their_key(ACTIVITY_ROWS_0077, "ac").await;
    let pool = state.db.clone().expect("a database");
    let event = |project: &str| ActivityEvent {
        time: chrono::Utc::now(),
        project: project.to_owned(),
        space: None,
        kind: "config.applied".into(),
        source: "reconciler".into(),
        summary: "Applied again.".into(),
        severity: "info".into(),
        correlation_id: None,
        details: Value::Null,
    };
    let mut tail = state.activity_events.subscribe(&orphan).await;
    state
        .activity
        .append(&[event(&placed), event(&orphan)])
        .await
        .expect("append");
    assert!(
        tail.try_recv().is_ok(),
        "the tail hears the event kept unkeyed"
    );
    let stamped: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT project, project_id::text FROM activity WHERE summary = 'Applied again.' \
         AND project = ANY($1) ORDER BY project = $2",
    )
    .bind(vec![placed.clone(), orphan.clone()])
    .bind(&orphan)
    .fetch_all(&pool)
    .await
    .expect("rows");
    assert_eq!(
        stamped,
        [
            (placed.clone(), Some(key.project.to_string())),
            (orphan.clone(), None)
        ]
    );
    let filter = ActivityFilter {
        limit: 10,
        ..ActivityFilter::default()
    };
    let unkeyed = state.activity.list(&orphan, &filter).await.expect("list");
    assert_eq!(
        unkeyed.items.len(),
        2,
        "a project with no key yet reads its events"
    );
    let page = state.activity.list(&placed, &filter).await.expect("list");
    assert_eq!(page.items.len(), 2, "the backfilled event and the new one");
    let later = placed_later(&state, &orphan, "aclate").await;
    let keyed: Vec<Option<String>> =
        sqlx::query_scalar("SELECT project_id::text FROM activity WHERE project = $1")
            .bind(&orphan)
            .fetch_all(&pool)
            .await
            .expect("rows");
    assert_eq!(keyed, vec![Some(later.project.to_string()); 2]);

    // The Organization's own events (people, roles) take its reserved project's key (TEN-11).
    let org = joinedcontext_portal::permissions::organization_of(&state.mirror, &placed)
        .expect("the placed project's Organization");
    let org_ns = joinedcontext_portal::permissions::ORG_NAMESPACE;
    state
        .mirror
        .upsert(envelope("Organization", &org, org_ns, json!({})));
    let reserved = project_ids::reserved(&pool, &org)
        .await
        .expect("reserved")
        .expect("a reserved key");
    let mut own = event(org_ns);
    own.summary = format!("People of {org} changed.");
    state.activity.append(&[own]).await.expect("append");
    let stamped: Option<String> =
        sqlx::query_scalar("SELECT project_id::text FROM activity WHERE summary = $1")
            .bind(format!("People of {org} changed."))
            .fetch_one(&pool)
            .await
            .expect("the Organization's event is kept");
    assert_eq!(stamped, Some(reserved.project.to_string()));
}

/// A workspace and a builder run of `project`, written as before the key (T-7206). `$1` is the
/// project.
const WORKSPACE_RUN_ROWS_0078: &[(&str, &str)] = &[
    (
        "workspaces",
        "INSERT INTO workspaces (name, project, owner, base_revision, scope, expires_at) \
         VALUES ('w' || left(md5($1), 12), $1, 'anna', 'abc123', '{\"kind\": \"project\"}', \
         now() + interval '1 hour')",
    ),
    (
        "agent_runs",
        "INSERT INTO agent_runs (id, project, app_name, endpoint_name, endpoint_slug, profile, \
         app_class, visibility, prompt, prompt_digest, data_needs, branch, path_prefix, status, \
         ticket_hash, created_by, expires_at) \
         VALUES ('run-' || md5($1), $1, 'kpi', 'air', 'air', 'builder', 'static', 'internal', 'p', \
         'd', '[]', 'b', 'apps/kpi', 'running', 'h', 'anna', now() + interval '1 hour')",
    ),
];

/// T-7206 PK-1g: the workspaces and builder runs of today are keyed by the backfill and read only
/// under their key; by name or id alone the stores find a placed project's row in its key and
/// none of a project without one, and refuse to open a workspace or a run there.
#[tokio::test]
async fn workspace_and_agent_run_rows_are_keyed_and_read_only_under_their_key() {
    let (state, placed, orphan, key) =
        rows_keyed_and_read_only_under_their_key(WORKSPACE_RUN_ROWS_0078, "wr").await;
    let pool = state.db.clone().expect("a database");
    let (placed_md5, orphan_md5): (String, String) = sqlx::query_as("SELECT md5($1), md5($2)")
        .bind(&placed)
        .bind(&orphan)
        .fetch_one(&pool)
        .await
        .expect("md5");
    let workspace = |md5: &str| format!("w{}", &md5[..12]);

    let found = state
        .workspaces
        .get(&workspace(&placed_md5))
        .await
        .expect("get")
        .expect("a placed project's workspace is found by name");
    assert_eq!(found.project, placed);
    assert!(state
        .workspaces
        .get(&workspace(&orphan_md5))
        .await
        .expect("get")
        .is_none());
    assert!(state
        .workspaces
        .list(&orphan)
        .await
        .expect("list")
        .is_empty());
    use joinedcontext_portal::ops::workspaces::{Opening, Scope};
    let opening = |name, project| Opening {
        name,
        title: None,
        project,
        owner: "anna",
        base_revision: "abc123",
        scope: Scope::Project {},
        ttl_hours: 1,
    };
    let refused = state
        .workspaces
        .create(opening("wr-orphan-new", &orphan))
        .await
        .expect_err("an unplaced project opens no workspace");
    assert!(
        refused.to_string().contains("Organizations page"),
        "{refused}"
    );
    let name = format!("x{}", &placed_md5[..12]);
    state
        .workspaces
        .create(opening(&name, &placed))
        .await
        .expect("a placed project opens a workspace");
    let stamped: Option<String> =
        sqlx::query_scalar("SELECT project_id::text FROM workspaces WHERE name = $1")
            .bind(&name)
            .fetch_one(&pool)
            .await
            .expect("row");
    assert_eq!(stamped, Some(key.project.to_string()));

    let run = state
        .agents
        .get_run(&format!("run-{placed_md5}"))
        .await
        .expect("get")
        .expect("a placed project's run is found by id");
    assert_eq!(run.project, placed);
    assert!(state
        .agents
        .get_run(&format!("run-{orphan_md5}"))
        .await
        .expect("get")
        .is_none());
    assert!(state
        .agents
        .list_runs(&orphan, 10)
        .await
        .expect("list")
        .is_empty());
    let mut orphan_run = run.clone();
    orphan_run.id = format!("run-new-{orphan_md5}");
    orphan_run.project = orphan.clone();
    let refused = state
        .agents
        .create_run(&orphan_run)
        .await
        .expect_err("an unplaced project starts no run");
    assert!(
        matches!(
            refused,
            joinedcontext_portal::agents::store::StoreError::Unplaced(_)
        ),
        "{refused:?}"
    );
    let mut placed_run = run;
    placed_run.id = format!("run-new-{placed_md5}");
    state.agents.create_run(&placed_run).await.expect("create");
    state
        .agents
        .set_title(&placed_run.id, "Air today")
        .await
        .expect("title");
    let titled: (Option<String>, Option<String>) =
        sqlx::query_as("SELECT title, project_id::text FROM agent_runs WHERE id = $1")
            .bind(&placed_run.id)
            .fetch_one(&pool)
            .await
            .expect("row");
    assert_eq!(
        titled,
        (Some("Air today".to_owned()), Some(key.project.to_string()))
    );
}

/// A row of each table T-7200 keys for `project`, written as before the key. `$1` is the project.
const PROJECT_ROWS_0071: &[(&str, &str)] = &[
    (
        "app_service_usage",
        "INSERT INTO app_service_usage (project, app, service, subject, day, used) \
         VALUES ($1, 'road-defects', 'email', '', current_date, 1)",
    ),
    (
        "email_unsubscribes",
        "INSERT INTO email_unsubscribes (project, app, subject) VALUES ($1, 'road-defects', 'sub-anna')",
    ),
    (
        "mqtt_capture",
        "INSERT INTO mqtt_capture (project, broker, topic, received_at, expires_at, bytes, payload) \
         VALUES ($1, 'cams', 'cams/1', now(), now() + interval '1 hour', 1, '\\x00')",
    ),
    (
        "entity_trash",
        "INSERT INTO entity_trash (project, space, owner, urn, type, entity) \
         VALUES ($1, 'air', 'f:1:anna', 'urn:ngsi-ld:T:x:1', 'T', '{}')",
    ),
    (
        "entity_comments",
        "INSERT INTO entity_comments (project, space, urn, author, author_name, body) \
         VALUES ($1, 'air', 'urn:ngsi-ld:T:x:1', 'anna', 'Anna', 'Looks off')",
    ),
    (
        "data_views",
        "INSERT INTO data_views (id, project, space, entity_type, kind, mode, title, owner_subject, owner_name, config) \
         VALUES ('dv-' || md5($1), $1, 'air', 'T', 'table', 'personal', 'Mine', 'sub-anna', 'Anna', '{}')",
    ),
];

/// T-7200 PK-1h: the App service counters, unsubscribes, MQTT capture, trash, comments and data
/// views of today are keyed by the backfill and read only under their key; the stores write new
/// rows in the key and keep none for a project without one.
#[tokio::test]
async fn app_capture_trash_comment_and_view_rows_are_keyed_and_read_only_under_their_key() {
    let (state, placed, orphan, key) =
        rows_keyed_and_read_only_under_their_key(PROJECT_ROWS_0071, "pr").await;
    let pool = state.db.clone().expect("a database");
    for (table, _) in PROJECT_ROWS_0071 {
        assert!(
            project_ids::KEYED_BY_NAME.contains(table),
            "{table} is backfilled"
        );
    }
    let entity = json!({ "id": "urn:ngsi-ld:T:x:2", "type": "T" });
    let kept = state
        .trash
        .keep(
            (&placed, "air", "f:1:sami"),
            "urn:ngsi-ld:T:x:2",
            "T",
            &entity,
        )
        .await
        .expect("keep")
        .expect("a placed project keeps its trash");
    let stamped: (Option<String>,) =
        sqlx::query_as("SELECT project_id::text FROM entity_trash WHERE id = $1")
            .bind(kept.id)
            .fetch_one(&pool)
            .await
            .expect("row");
    assert_eq!(stamped.0, Some(key.project.to_string()));
    assert_eq!(
        state
            .trash
            .keep(
                (&orphan, "air", "f:1:sami"),
                "urn:ngsi-ld:T:x:2",
                "T",
                &entity
            )
            .await
            .expect("keep"),
        None,
        "a project with no Organization of the registry keeps no trash"
    );
    assert!(state
        .trash
        .list((&orphan, "air", "f:1:anna"))
        .await
        .expect("list")
        .is_empty());
    assert_eq!(
        state
            .trash
            .list((&placed, "air", "f:1:anna"))
            .await
            .expect("list")
            .len(),
        1,
        "the backfilled row reads in its key"
    );
}

/// A row of each table T-7202 keys for `project`, written as before the key. `$1` is the
/// project; the access request asks for an Endpoint of the same project, its key column.
const PROJECT_ROWS_0073: &[(&str, &str)] = &[
    (
        "erasures",
        "INSERT INTO erasures (id, project, space, subject, reason, actor, groups, stores, approval) \
         VALUES ('er-' || md5($1), $1, 'air', md5('jana'), 'asked', 'user:anna', '[]', \
                 '{\"broker\": {\"state\": \"pending\"}}', 'approved')",
    ),
    (
        "access_requests",
        "INSERT INTO access_requests (id, project, pipeline, endpoint_project, endpoint, space, \
           operations, types, reason, state, requested_by) \
         VALUES ('ar-' || substr(md5($1), 1, 12), $1, 'air-index', $1, 'air', 'air', 'read', '[]', \
                 'kpis', 'pending', 'anna')",
    ),
];

/// T-7202 PK-1i: the erasure log and the access requests of today are keyed by the backfill and
/// read only under their key; an access request takes the key of the Endpoint's project; an
/// erasure of a project that left the mirror still answers the minute's pass, so it finishes.
#[tokio::test]
async fn erasure_and_access_request_rows_are_keyed_and_read_only_under_their_key() {
    let (state, placed, orphan, key) =
        rows_keyed_and_read_only_under_their_key(PROJECT_ROWS_0073, "pi").await;
    let pool = state.db.clone().expect("a database");
    assert_eq!(
        project_ids::key_column("access_requests"),
        "endpoint_project"
    );
    assert!(project_ids::LEFT_UNKEYED
        .iter()
        .all(|(t, _)| !project_ids::KEYED_BY_NAME.contains(t)));

    // A request a pipeline of another project files is keyed by the Endpoint's project and still
    // lists for the asking pipeline.
    let asking = run_name("piasking");
    let mut tx = joinedcontext_portal::db::tenant_tx(&pool, key)
        .await
        .expect("tx");
    sqlx::query(
        "INSERT INTO access_requests (id, project, pipeline, endpoint_project, endpoint, space, \
           operations, types, reason, state, requested_by) \
         VALUES ('ar-' || substr(md5($1), 1, 12), $1, 'air-index', $2, 'air', 'air', 'read', '[]', \
                 'kpis', 'pending', 'anna')",
    )
    .bind(&asking)
    .bind(&placed)
    .execute(&mut *tx)
    .await
    .expect("a request into the placed project");
    tx.commit().await.expect("commit");
    let stamped: Option<String> =
        sqlx::query_scalar("SELECT project_id::text FROM access_requests WHERE project = $1")
            .bind(&asking)
            .fetch_one(&pool)
            .await
            .expect("row");
    assert_eq!(stamped, Some(key.project.to_string()));
    let outgoing = joinedcontext_portal::access_requests::outgoing(&pool, &asking, "air-index")
        .await
        .expect("outgoing");
    assert_eq!(outgoing.len(), 1, "{outgoing:?}");
    assert_eq!(outgoing[0].endpoint_project, placed);
    assert_eq!(
        joinedcontext_portal::access_requests::incoming(&pool, &orphan)
            .await
            .expect("incoming")
            .len(),
        1,
        "an unkeyed request still reads while the forcing migration waits"
    );

    // The placed project leaves the mirror: its approved erasure with a pending store is still
    // open, in the key the registry recorded.
    state
        .mirror
        .remove(&joinedcontext_portal::resource::ResourceKey {
            namespace: joinedcontext_portal::permissions::ORG_NAMESPACE.to_owned(),
            kind: "Project".to_owned(),
            name: placed.clone(),
        });
    assert_eq!(
        project_ids::key_of(&pool, &state.mirror, &placed)
            .await
            .expect("key_of"),
        None
    );
    assert_eq!(
        project_ids::recorded(&pool, &placed)
            .await
            .expect("recorded"),
        Some(key)
    );
    let open = joinedcontext_portal::erasure::open(&pool)
        .await
        .expect("open");
    for project in [&placed, &orphan] {
        assert_eq!(
            open.iter().filter(|e| &e.project == project).count(),
            1,
            "{project}: one open erasure, read once: {open:?}"
        );
    }
}

const PIPELINE_ROWS_0072: &[(&str, &str)] = &[
    (
        "pipeline_log",
        "INSERT INTO pipeline_log (project, pipeline, run, outcome) VALUES ($1, 'trees', 'r1', 'sent')",
    ),
    (
        "pipeline_runs",
        "INSERT INTO pipeline_runs (project, pipeline, run, sent) VALUES ($1, 'trees', 'r1', 1)",
    ),
    (
        "pipeline_rejected",
        "INSERT INTO pipeline_rejected (project, pipeline, record, rule, message) \
         VALUES ($1, 'trees', '{}', 'required', 'height is missing')",
    ),
    (
        "pipeline_status",
        "INSERT INTO pipeline_status (project, pipeline, status) VALUES ($1, 'trees', '{}')",
    ),
    (
        "pipeline_templates",
        "INSERT INTO pipeline_templates (id, project, title, spec, created_by) \
         VALUES ('tpl-' || md5($1), $1, 'Trees', '{}', 'anna')",
    ),
    (
        "pipeline_record_hashes",
        "INSERT INTO pipeline_record_hashes (project, pipeline, record, hash) VALUES ($1, 'trees', 'r', 'h')",
    ),
    (
        "pipeline_source_ledger",
        "INSERT INTO pipeline_source_ledger (project, pipeline, entity_id, last_run) \
         VALUES ($1, 'trees', 'urn:ngsi-ld:Tree:1', 'r1')",
    ),
    (
        "pipeline_source_holds",
        "INSERT INTO pipeline_source_holds (project, pipeline, run, reason, would, ledger) \
         VALUES ($1, 'trees', 'r1', 'empty', 0, 1)",
    ),
    (
        "pipeline_source_complete",
        "INSERT INTO pipeline_source_complete (project, pipeline, run) VALUES ($1, 'trees', 'r1')",
    ),
    (
        "pipeline_source_removed",
        "INSERT INTO pipeline_source_removed (project, space, entity_id, pipeline, run) \
         VALUES ($1, 'air', 'urn:ngsi-ld:Tree:1', 'trees', 'r1')",
    ),
];

/// T-7198 PK-1f: the run log, rejects, status, templates, record hashes and source ledger of
/// today are keyed by the backfill and read only under their key; the log writes new lines in the
/// key, and a project with no key yet keeps and reads its runs unkeyed in phase one, the passes
/// over every project included, until the backfill keys them.
#[tokio::test]
async fn pipeline_rows_are_keyed_and_read_only_under_their_key() {
    let (state, placed, orphan, key) =
        rows_keyed_and_read_only_under_their_key(PIPELINE_ROWS_0072, "pl").await;
    let pool = state.db.clone().expect("a database");
    for (table, _) in PIPELINE_ROWS_0072 {
        assert!(
            project_ids::KEYED_BY_NAME.contains(table),
            "{table} is backfilled"
        );
    }
    let line = joinedcontext_portal::pipeline_log::NewLine {
        record_id: "urn:ngsi-ld:Tree:2".into(),
        step: None,
        outcome: joinedcontext_portal::pipeline_log::Outcome::Sent,
        message: String::new(),
    };
    for project in [&placed, &orphan] {
        state
            .pipeline_log
            .append(project, "trees", "r2", std::slice::from_ref(&line))
            .await
            .expect("append");
    }
    let stamped: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT project, project_id::text FROM pipeline_log WHERE run = 'r2' AND project = ANY($1) \
         ORDER BY project = $2",
    )
    .bind(vec![placed.clone(), orphan.clone()])
    .bind(&orphan)
    .fetch_all(&pool)
    .await
    .expect("rows");
    assert_eq!(
        stamped,
        [
            (placed.clone(), Some(key.project.to_string())),
            (orphan.clone(), None)
        ],
        "a placed project's line lands in its key and a project with no key yet keeps it unkeyed"
    );
    assert_eq!(
        state
            .pipeline_log
            .runs(&orphan, "trees", 10)
            .await
            .expect("runs")
            .len(),
        2,
        "a project with no key yet reads its runs (the train smoke's run history)"
    );
    let ran = state.pipeline_log.ran().await.expect("ran");
    for project in [&placed, &orphan] {
        assert!(
            ran.contains_key(&(project.clone(), "trees".to_owned())),
            "the pass over every project reads {project}"
        );
    }
    assert_eq!(
        state
            .pipeline_log
            .runs(&placed, "trees", 10)
            .await
            .expect("runs")
            .len(),
        2,
        "the backfilled run and the new one read in their key"
    );
}
