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
            .contains("keeps its name, its key and its id"),
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
