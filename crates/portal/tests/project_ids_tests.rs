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

/// CHG-15 (T-7212, migration 0082): a branch row needs a parented project, renews only while it
/// lives, by at most 7 days at a time and to at most 30 days from creation, keeps its creation,
/// and is never deleted, so an expired id stays a tombstone; the router reads it with its expiry.
#[tokio::test]
async fn a_branch_lives_renews_and_stays_a_tombstone() {
    let pool = pool().await;
    let org = organization(&pool, "branch-life").await;
    let parent = ProjectId::mint();
    let branch = ProjectId::mint();
    let expired = ProjectId::mint();
    for (id, name, parent) in [
        (parent, run_name("branch-parent"), None),
        (branch, format!("b-{}", branch.path_segment()), Some(parent)),
        (
            expired,
            format!("b-{}", expired.path_segment()),
            Some(parent),
        ),
    ] {
        sqlx::query(
            "INSERT INTO project_ids (id, name, parent, organization) VALUES ($1::uuid, $2, $3::uuid, $4)",
        )
        .bind(id.to_string())
        .bind(name)
        .bind(parent.map(|p| p.to_string()))
        .bind(&org)
        .execute(&pool)
        .await
        .expect("a project row");
    }
    let refused = |statement: &'static str, id: ProjectId| {
        let pool = pool.clone();
        async move {
            sqlx::query(statement)
                .bind(id.to_string())
                .execute(&pool)
                .await
                .expect_err(statement)
                .to_string()
        }
    };
    let no_parent = refused(
        "INSERT INTO project_branches (id, created_by, expires_at) VALUES ($1::uuid, 'ana', now() + interval '7 days')",
        parent,
    )
    .await;
    assert!(no_parent.contains("no branch"), "{no_parent}");
    sqlx::query(
        "INSERT INTO project_branches (id, created_by, expires_at) VALUES ($1::uuid, 'ana', now() + interval '7 days')",
    )
    .bind(branch.to_string())
    .execute(&pool)
    .await
    .expect("a branch");
    sqlx::query(
        "INSERT INTO project_branches (id, created_by, created_at, expires_at) \
         VALUES ($1::uuid, 'ana', now() - interval '8 days', now() - interval '1 day')",
    )
    .bind(expired.to_string())
    .execute(&pool)
    .await
    .expect("an expired branch");
    let renewed = "UPDATE project_branches SET expires_at = expires_at + interval '7 days' WHERE id = $1::uuid";
    sqlx::query(renewed)
        .bind(branch.to_string())
        .execute(&pool)
        .await
        .expect("a live branch renews by 7 days");
    for (statement, id, says) in [
        (renewed, expired, "only while it lives"),
        (
            "UPDATE project_branches SET expires_at = expires_at + interval '8 days' WHERE id = $1::uuid",
            branch,
            "at most 7 days",
        ),
        (
            "UPDATE project_branches SET expires_at = created_at + interval '31 days' WHERE id = $1::uuid",
            branch,
            "renewed",
        ),
        (
            "UPDATE project_branches SET expires_at = expires_at - interval '1 day' WHERE id = $1::uuid",
            branch,
            "at most 7 days",
        ),
        (
            "UPDATE project_branches SET created_by = 'eve' WHERE id = $1::uuid",
            branch,
            "keeps its creation",
        ),
        ("DELETE FROM project_branches WHERE id = $1::uuid", expired, "tombstone"),
    ] {
        let err = refused(statement, id).await;
        assert!(err.contains(says), "{statement}: {err}");
    }
    let listed = project_ids::every_branch_owned(&pool)
        .await
        .expect("branches");
    let mine: Vec<_> = listed.iter().filter(|(o, ..)| *o == org).collect();
    assert_eq!(mine.len(), 2, "{mine:?}");
    let now = jiff::Timestamp::now().as_second();
    for (_, name, key, expires_at) in mine {
        assert_eq!(*name, format!("b-{}", key.project.path_segment()));
        if key.project == expired {
            assert!(*expires_at < now);
        } else {
            assert_eq!(key.project, branch);
            assert!(*expires_at > now + 13 * 86_400);
        }
    }
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

/// T-7458: a project whose Pipelines run with no active Organization of the registry is named
/// with its count of Pipelines; a placed project and one with no Pipeline are not.
#[tokio::test]
async fn the_projects_with_pipelines_and_no_organization_are_named() {
    let pool = pool().await;
    let state = AppState::new(Config::for_tests(), None).with_db(pool.clone());
    let placed = run_name("placed");
    let unplaced = run_name("unplaced");
    let empty = run_name("empty");
    common::keyed_project(&state, &pool, &placed, &run_name("home")).await;
    for project in [&unplaced, &empty] {
        state.mirror.upsert(envelope(
            "Project",
            project,
            joinedcontext_portal::permissions::ORG_NAMESPACE,
            json!({ "organizationRef": run_name("nowhere") }),
        ));
    }
    for (project, pipeline) in [(&placed, "aq"), (&unplaced, "aq"), (&unplaced, "trees")] {
        state
            .mirror
            .upsert(envelope("Pipeline", pipeline, project, json!({})));
    }
    let named: Vec<(String, usize)> =
        joinedcontext_portal::host_registry::unplaced_pipelines(&pool, &state.mirror)
            .await
            .expect("the unplaced projects")
            .into_iter()
            .filter(|(project, _)| [&placed, &unplaced, &empty].contains(&project))
            .collect();
    assert_eq!(named, [(unplaced.clone(), 2)]);
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

const KEYED: [&str; 8] = [
    "organization_backup_targets",
    "organization_backup_log",
    "organization_data_restores",
    "agent_run_events",
    "sync_source_state",
    "connectors",
    "connector_calls",
    "user_preferences",
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
    let policies: Vec<(String, String, String, String, bool)> = sqlx::query_as(
        "SELECT p.tablename::text, p.policyname::text, coalesce(p.qual, ''), \
                coalesce(p.with_check, ''), \
                p.roles = ARRAY[pg_get_userbyid(c.relowner)]::name[] \
         FROM pg_policies p JOIN pg_namespace n ON n.nspname = p.schemaname \
         JOIN pg_class c ON c.relnamespace = n.oid AND c.relname = p.tablename \
         WHERE p.schemaname = 'public'",
    )
    .fetch_all(&pool)
    .await
    .expect("policies");
    for (table, name, using, check, to_owner) in &policies {
        // The reviewed exception (T-7476): the move's two policies apply only to the table's
        // owner inside a SECURITY DEFINER call (session_user <> current_user) and narrow to the
        // one project the function moves. Exempt by exact name and shape only.
        let move_policy = match name.as_str() {
            "project_move_read" => Some(check.is_empty()),
            "project_move" => Some(!check.is_empty()),
            _ => None,
        };
        if let Some(shape) = move_policy {
            for text in [using, check].into_iter().filter(|t| !t.is_empty()) {
                assert!(
                    *to_owner
                        && shape
                        && text.starts_with("((SESSION_USER <> CURRENT_USER) AND ")
                        && settings_read(text) == ["jc.project_move"],
                    "{table}.{name}: a move policy is TO the table owner, gated on \
                     session_user <> current_user and reads jc.project_move alone: {text}"
                );
            }
            continue;
        }
        for text in [using, check] {
            let settings = settings_read(text);
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

/// The `current_setting('...')` names a policy expression reads.
fn settings_read(text: &str) -> Vec<&str> {
    text.match_indices("current_setting('")
        .map(|(at, m)| text[at + m.len()..].split('\'').next().unwrap_or(""))
        .collect()
}

/// Runs a test's CREATE ROLE / GRANT statements. Roles live in the cluster's shared catalog, so
/// parallel tests granting at once can fail with XX000 "tuple concurrently updated", even on
/// roles of their own (each test has its own database, so an advisory lock would not serialize
/// them): try again a few times.
async fn setup_role(pool: &sqlx::PgPool, sql: &'static str) {
    for attempt in 1.. {
        match sqlx::raw_sql(sql).execute(pool).await {
            Ok(_) => return,
            Err(sqlx::Error::Database(e))
                if e.code().as_deref() == Some("XX000") && attempt < 5 =>
            {
                tokio::time::sleep(std::time::Duration::from_millis(50 * attempt)).await;
            }
            Err(e) => panic!("the role: {e:?}"),
        }
    }
}

/// A transaction as the role `pk_rls_reader`, which owns nothing and has no BYPASSRLS, as the
/// serving role of T-7179; `key` set as `db::tenant_tx` sets it.
async fn as_reader(
    pool: &sqlx::PgPool,
    key: Option<jc_types::ProjectKey>,
) -> sqlx::Transaction<'static, sqlx::Postgres> {
    setup_role(
        pool,
        "DO $$ BEGIN CREATE ROLE pk_rls_reader NOLOGIN NOBYPASSRLS; \
         EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$; \
         GRANT SELECT, INSERT, UPDATE, DELETE ON organization_backup_log TO pk_rls_reader; \
         GRANT USAGE ON ALL SEQUENCES IN SCHEMA public TO pk_rls_reader",
    )
    .await;
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
/// S1-27 (T-7278): a run's events answer only under its run's key. Before, they were read on the
/// bare pool by run id, so another Organization reading by that id got the whole stream; and a
/// write under another key never lands on the run.
#[tokio::test]
async fn a_runs_events_answer_only_under_its_organizations_key() {
    let pool = pool().await;
    let org = organization(&pool, "evorg").await;
    let other = organization(&pool, "evother").await;
    let key = project_ids::key(&pool, &org, &run_name("evp"))
        .await
        .expect("key")
        .expect("a key");
    let other_key = project_ids::key(&pool, &other, &run_name("evq"))
        .await
        .expect("key")
        .expect("a key");
    // A role of its own: GRANTs racing on one role's catalog rows fail a parallel test.
    setup_role(
        &pool,
        "DO $$ BEGIN CREATE ROLE ev_rls_reader NOLOGIN NOBYPASSRLS; \
         EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$; \
         GRANT SELECT, INSERT, UPDATE, DELETE ON agent_runs, agent_run_events TO ev_rls_reader",
    )
    .await;
    let as_reader = |pool: sqlx::PgPool, key: Option<jc_types::ProjectKey>| async move {
        let mut tx = match key {
            Some(key) => joinedcontext_portal::db::tenant_tx(&pool, key)
                .await
                .expect("keyed"),
            None => pool.begin().await.expect("begin"),
        };
        sqlx::query("SET LOCAL ROLE ev_rls_reader")
            .execute(&mut *tx)
            .await
            .expect("role");
        tx
    };
    let run = run_name("run");
    let mut tx = as_reader(pool.clone(), Some(key)).await;
    sqlx::query(
        "INSERT INTO agent_runs (id, project, app_name, endpoint_name, endpoint_slug, profile, \
         app_class, visibility, prompt, prompt_digest, data_needs, branch, path_prefix, status, \
         ticket_hash, created_by, expires_at) VALUES ($1, 'evp', 'app', 'e', 's', 'default', \
         'static', 'private', 'p', 'd', '[]', 'b', 'apps/app', 'running', '', 'test', \
         now() + interval '1 hour')",
    )
    .bind(&run)
    .execute(&mut *tx)
    .await
    .expect("a run under its key");
    joinedcontext_portal::db::append_agent_run_event(&mut tx, &run, "status", &json!({"s": 1}))
        .await
        .expect("an event under the run's key");
    tx.commit().await.expect("commit");

    let read = |key: Option<jc_types::ProjectKey>| {
        let (pool, run) = (pool.clone(), run.clone());
        async move {
            let mut tx = as_reader(pool, key).await;
            let since = joinedcontext_portal::db::load_agent_run_events(&mut tx, &run, 0)
                .await
                .expect("read")
                .len();
            let of = joinedcontext_portal::db::load_agent_run_events_of(
                &mut tx,
                std::slice::from_ref(&run),
                &["status"],
            )
            .await
            .expect("read")
            .len();
            (since, of)
        }
    };
    assert_eq!(read(Some(key)).await, (1, 1));
    assert_eq!(
        read(Some(other_key)).await,
        (0, 0),
        "another Organization reads none of the run's events"
    );
    assert_eq!(read(None).await, (0, 0), "the key unset reads no event");

    let mut tx = as_reader(pool.clone(), Some(other_key)).await;
    assert!(
        joinedcontext_portal::db::append_agent_run_event(&mut tx, &run, "status", &json!({}))
            .await
            .is_err(),
        "another Organization's key extends no run it cannot see"
    );
}

/// S1-27 (T-7281): a project's sync memory answers only under its project's key. Before, it was
/// read and written on the bare pool by project name, so another Organization read or overwrote
/// it; `States` now opens the project's tenant_tx from the mirror.
#[tokio::test]
async fn sync_state_answers_only_under_its_projects_key() {
    use joinedcontext_portal::db::{load_sync_state, save_sync_state, SyncStateRow};
    use joinedcontext_portal::sync::state::{States, Stored};
    let pool = pool().await;
    let org = organization(&pool, "syorg").await;
    let other = organization(&pool, "syother").await;
    let (project, foreign) = (run_name("syp"), run_name("syq"));
    let mirror = std::sync::Arc::new(joinedcontext_portal::store::Mirror::default());
    for (name, organization) in [(&project, &org), (&foreign, &other)] {
        mirror.upsert(envelope(
            "Project",
            name,
            joinedcontext_portal::permissions::ORG_NAMESPACE,
            json!({ "organizationRef": organization }),
        ));
    }
    let key = project_ids::key(&pool, &org, &project)
        .await
        .expect("key")
        .expect("a key");
    let other_key = project_ids::key(&pool, &other, &foreign)
        .await
        .expect("key")
        .expect("a key");
    let states = States::new(Some((pool.clone(), std::sync::Arc::clone(&mirror))));
    let mut stored = Stored::default();
    stored.state.observed_revision = Some("c0ffee".into());
    states.put(&project, "regional", &stored).await;
    assert_eq!(states.get(&project, "regional").await, stored);
    assert_eq!(
        states.get(&foreign, "regional").await,
        Stored::default(),
        "another project's source of the same name is its own"
    );
    assert_eq!(
        states.get("no-such-project", "regional").await,
        Stored::default()
    );

    // A role of its own: GRANTs racing on one role's catalog rows fail a parallel test.
    setup_role(
        &pool,
        "DO $$ BEGIN CREATE ROLE sy_rls_reader NOLOGIN NOBYPASSRLS; \
         EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$; \
         GRANT SELECT, INSERT, UPDATE, DELETE ON sync_source_state TO sy_rls_reader",
    )
    .await;
    let as_reader = |pool: sqlx::PgPool, key: Option<jc_types::ProjectKey>| async move {
        let mut tx = match key {
            Some(key) => joinedcontext_portal::db::tenant_tx(&pool, key)
                .await
                .expect("keyed"),
            None => pool.begin().await.expect("begin"),
        };
        sqlx::query("SET LOCAL ROLE sy_rls_reader")
            .execute(&mut *tx)
            .await
            .expect("role");
        tx
    };
    let read = |key: Option<jc_types::ProjectKey>| {
        let (pool, project) = (pool.clone(), project.clone());
        async move {
            let mut tx = as_reader(pool, key).await;
            load_sync_state(&mut tx, &project, "regional")
                .await
                .expect("read")
                .and_then(|row| row.observed_revision)
        }
    };
    assert_eq!(read(Some(key)).await.as_deref(), Some("c0ffee"));
    assert_eq!(
        read(Some(other_key)).await,
        None,
        "another Organization reads no sync state of the project"
    );
    assert_eq!(read(None).await, None, "the key unset reads no row");

    let mut tx = as_reader(pool.clone(), Some(other_key)).await;
    let overwrite = SyncStateRow {
        namespace: project.clone(),
        name: "regional".into(),
        observed_revision: Some("evil".into()),
        ..SyncStateRow::default()
    };
    assert!(
        save_sync_state(&mut tx, &overwrite).await.is_err(),
        "another Organization's key overwrites no row it cannot see"
    );
    drop(tx);
    assert_eq!(read(Some(key)).await.as_deref(), Some("c0ffee"));
}

/// T-7319 (jc-prod): the forge's installation claim names an Organization the registry lacks
/// (`hel` beside a registry of smartcity, marek-mraz, bbsk, slovensko), so the org namespace had no
/// key, no check of an org draft was kept and every proposal into org answered "has not been
/// checked". The configured installation Organization keys it then; a claim the registry holds
/// as active still wins, and an inactive one falls back again.
#[tokio::test]
async fn the_org_namespace_takes_the_configured_installation_when_the_claim_has_no_registry_row() {
    use joinedcontext_portal::ops::verdict::Verdict;
    let pool = pool().await;
    let org_ns = joinedcontext_portal::permissions::ORG_NAMESPACE;
    let configured = organization(&pool, "cfginst").await;
    let claim = run_name("cfghel");
    let mirror = std::sync::Arc::new(joinedcontext_portal::store::Mirror::default());
    mirror.upsert(envelope(
        "Organization",
        &claim,
        org_ns,
        json!({ "installation": true }),
    ));
    mirror.upsert(envelope(
        "Organization",
        &run_name("cfgzil"),
        org_ns,
        json!({}),
    ));
    let reserved = |name: String| {
        let pool = pool.clone();
        async move {
            project_ids::reserved(&pool, &name)
                .await
                .expect("reserved")
                .expect("an active Organization's key")
        }
    };

    assert_eq!(
        project_ids::key_of(&pool, &mirror, org_ns)
            .await
            .expect("key"),
        None,
        "without a configured installation an unknown claim keys nothing"
    );
    mirror.set_configured_installation(&configured);
    mirror.set_configured_installation("ignored-second-call");
    let fallback = reserved(configured.clone()).await;
    assert_eq!(
        project_ids::key_of(&pool, &mirror, org_ns)
            .await
            .expect("key"),
        Some(fallback)
    );
    assert_eq!(
        project_ids::installation(&pool, &mirror)
            .await
            .expect("installation"),
        Some(fallback)
    );

    // A check of an org draft is kept, so the proposal that follows finds its verdict.
    let state = AppState::new(Config::for_tests(), None)
        .with_db(pool.clone())
        .with_mirror(std::sync::Arc::clone(&mirror));
    let binding = run_name("cfgrb");
    let manifest = json!({ "kind": "RoleBinding", "metadata": { "name": binding } });
    state
        .drafts
        .put(
            org_ns,
            "RoleBinding",
            &binding,
            manifest.clone(),
            None,
            "anna",
            "human",
        )
        .await
        .expect("an org draft is kept under the configured installation");
    state
        .drafts
        .set_verdict(
            org_ns,
            "RoleBinding",
            &binding,
            Verdict::new(true, vec![], None, &manifest),
        )
        .await
        .expect("its check is kept");
    let draft = state
        .drafts
        .get(org_ns, "RoleBinding", &binding)
        .await
        .expect("get")
        .expect("the draft");
    assert!(draft
        .verdict
        .is_some_and(|v| v.ok && v.is_fresh_for(&manifest)));

    // The registry holds the claim: the claim wins over the configured one.
    sqlx::query(
        "INSERT INTO organizations (name, host, state, created_by) VALUES ($1, $1 || '.example', 'active', 'test')",
    )
    .bind(&claim)
    .execute(&pool)
    .await
    .expect("the registry holds the claim");
    let claimed = reserved(claim.clone()).await;
    assert_ne!(claimed, fallback);
    assert_eq!(
        project_ids::key_of(&pool, &mirror, org_ns)
            .await
            .expect("key"),
        Some(claimed)
    );
    // Removed from the registry: the configured one again, never the inactive claim.
    sqlx::query("UPDATE organizations SET state = 'removed' WHERE name = $1")
        .bind(&claim)
        .execute(&pool)
        .await
        .expect("removed");
    assert_eq!(
        project_ids::key_of(&pool, &mirror, org_ns)
            .await
            .expect("key"),
        Some(fallback)
    );
}

/// T-7414 (jc-prod): the forge's installation claim (`hel`) has no registry row and the deployment
/// configures `smartcity`. The installation's projects key under the configured Organization, a
/// person on the deployment's own host keeps a connector there (was 409 "belongs to no active
/// Organization"), and a project of another Organization the registry lacks still keys nothing.
#[tokio::test]
async fn the_installation_claim_without_a_registry_row_keys_under_the_configured_one() {
    use joinedcontext_portal::connectors::{ConnectorStore, NewConnector};
    let pool = pool().await;
    let org_ns = joinedcontext_portal::permissions::ORG_NAMESPACE;
    let configured = organization(&pool, "t14city").await;
    let (claim, other) = (run_name("t14hel"), run_name("t14zil"));
    let (own, foreign) = (run_name("t14helsinki"), run_name("t14zilina"));
    let mirror = std::sync::Arc::new(joinedcontext_portal::store::Mirror::default());
    mirror.upsert(envelope(
        "Organization",
        &claim,
        org_ns,
        json!({ "installation": true }),
    ));
    mirror.upsert(envelope("Organization", &other, org_ns, json!({})));
    mirror.upsert(envelope("Project", &own, org_ns, json!({})));
    mirror.upsert(envelope(
        "Project",
        &foreign,
        org_ns,
        json!({ "organizationRef": other }),
    ));
    mirror.set_configured_installation(&configured);

    let expected = project_ids::key(&pool, &configured, &own)
        .await
        .expect("key")
        .expect("the configured installation's key");
    assert_eq!(
        project_ids::key_of(&pool, &mirror, &own)
            .await
            .expect("key"),
        Some(expected)
    );
    assert_eq!(
        project_ids::key_of(&pool, &mirror, &foreign)
            .await
            .expect("key"),
        None,
        "another Organization the registry lacks never takes the installation's key"
    );

    let store = ConnectorStore::new(Some((pool.clone(), std::sync::Arc::clone(&mirror))), None);
    let ask: NewConnector = serde_json::from_value(json!({
        "name": "smartcity-mcp", "segments": ["data-read"], "scope": [],
        "expiresAt": "2030-01-01T00:00:00Z"
    }))
    .expect("a connector");
    let made = store
        .create(
            &run_name("f:1:city-admin"),
            ask,
            vec![],
            "https://portal.test",
        )
        .await
        .expect("created on the deployment's own host");
    let reserved = project_ids::reserved(&pool, &configured)
        .await
        .expect("reserved")
        .expect("the configured installation's reserved key");
    let (org_id, project_id): (String, String) =
        sqlx::query_as("SELECT org_id::text, project_id::text FROM connectors WHERE id = $1")
            .bind(&made.id)
            .fetch_one(&pool)
            .await
            .expect("the row");
    assert_eq!(
        (org_id, project_id),
        (reserved.org.to_string(), reserved.project.to_string())
    );
}

/// T-7319: the drift report names each side once the mirror holds Organizations, and nothing
/// before the first sync.
#[tokio::test]
async fn the_organization_drift_names_each_side_of_the_forge_and_the_registry() {
    use joinedcontext_portal::host_registry::report_drift;
    let pool = pool().await;
    let org_ns = joinedcontext_portal::permissions::ORG_NAMESPACE;
    let both = organization(&pool, "drboth").await;
    let registry_only = organization(&pool, "drreg").await;
    let forge_only = run_name("drforge");
    let mirror = joinedcontext_portal::store::Mirror::default();
    assert_eq!(report_drift(&pool, &mirror).await.expect("drift"), None);
    mirror.upsert(envelope("Organization", &both, org_ns, json!({})));
    mirror.upsert(envelope("Organization", &forge_only, org_ns, json!({})));
    let drift = report_drift(&pool, &mirror)
        .await
        .expect("drift")
        .expect("a loaded mirror reports");
    assert!(
        drift.missing_from_forge.contains(&registry_only),
        "{drift:?}"
    );
    assert!(!drift.missing_from_forge.contains(&both), "{drift:?}");
    assert_eq!(drift.missing_from_registry, [forge_only], "{drift:?}");
}

/// T-7282, the smoke's multiorg@path: on a host Organizations share by path (no Organization host,
/// no `JC_PORTAL_INSTALLATION_ORGANIZATION`), a person's connector takes the key of the
/// installation Organization the mirror names, once the registry holds it; until then the create
/// answers 409 with the next step instead of writing an unkeyed row.
#[tokio::test]
async fn a_shared_hosts_connector_takes_the_mirrors_installation_organization() {
    use joinedcontext_portal::connectors::{ConnectorStore, NewConnector};
    let pool = pool().await;
    let (ops, hel) = (run_name("psops"), run_name("pshel"));
    let mirror = std::sync::Arc::new(joinedcontext_portal::store::Mirror::default());
    let org_ns = joinedcontext_portal::permissions::ORG_NAMESPACE;
    mirror.upsert(envelope(
        "Organization",
        &ops,
        org_ns,
        json!({ "installation": true }),
    ));
    mirror.upsert(envelope("Organization", &hel, org_ns, json!({})));
    let store = ConnectorStore::new(Some((pool.clone(), std::sync::Arc::clone(&mirror))), None);
    let ask = || -> NewConnector {
        serde_json::from_value(json!({
            "name": "doprava", "segments": ["data-read"], "scope": [],
            "expiresAt": "2030-01-01T00:00:00Z"
        }))
        .expect("a connector")
    };
    let (owner, base) = (run_name("f:1:hel-admin"), "https://portal.test");

    let refused = store.create(&owner, ask(), vec![], base).await;
    assert!(
        matches!(&refused, Err(joinedcontext_portal::error::ApiError::Conflict(m)) if m.contains("installation Organization")),
        "an installation Organization the registry lacks keys nothing: {refused:?}"
    );
    sqlx::query(
        "INSERT INTO organizations (name, host, state, created_by) VALUES ($1, $1 || '.example', 'active', 'test')",
    )
    .bind(&ops)
    .execute(&pool)
    .await
    .expect("the registry holds ops");
    let made = store
        .create(&owner, ask(), vec![], base)
        .await
        .expect("created on the shared host");
    let ops_key = project_ids::reserved(&pool, &ops)
        .await
        .expect("reserved")
        .expect("ops' key");
    let (org_id, project_id): (String, String) =
        sqlx::query_as("SELECT org_id::text, project_id::text FROM connectors WHERE id = $1")
            .bind(&made.id)
            .fetch_one(&pool)
            .await
            .expect("the row");
    assert_eq!(
        (org_id, project_id),
        (ops_key.org.to_string(), ops_key.project.to_string())
    );
    let listed = store.of_owner(&owner, base).await.expect("list");
    assert_eq!(
        listed.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        [made.id.as_str()]
    );
}

/// S1-27 (T-7282): a person's connectors, their calls and preferences answer only in the
/// Organization the person used the Portal in. Before, they were read on the bare pool by subject
/// or id, so a request on one Organization's host read, revoked or overwrote another's.
#[tokio::test]
async fn a_persons_rows_answer_only_in_their_organization() {
    use joinedcontext_portal::connectors::{Call, ConnectorStore, NewConnector};
    use joinedcontext_portal::organization_host::{scope, OrganizationHost};
    let pool = pool().await;
    let (a, b) = (
        organization(&pool, "pra").await,
        organization(&pool, "prb").await,
    );
    let host = |org: &str| OrganizationHost {
        organization: org.to_owned(),
        base: url::Url::parse(&format!("https://{org}.example")).expect("url"),
    };
    let ask = || -> NewConnector {
        serde_json::from_value(json!({
            "name": "doprava", "segments": ["data-read"], "scope": [],
            "expiresAt": "2030-01-01T00:00:00Z"
        }))
        .expect("a connector")
    };
    let owner = run_name("f:1:ana");
    let mirror = std::sync::Arc::new(joinedcontext_portal::store::Mirror::default());
    // The store serves as the Portal does: a role row-level security binds (the test's own
    // connection is a superuser, which bypasses it).
    let admin = pool;
    setup_role(
        &admin,
        "DO $$ BEGIN CREATE ROLE pr_serving LOGIN PASSWORD 'pr-serving' NOBYPASSRLS; \
         EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$; \
         GRANT SELECT, INSERT, UPDATE, DELETE ON connectors, connector_calls, user_preferences, \
               organizations, project_ids TO pr_serving; \
         GRANT USAGE ON SEQUENCE connector_calls_id_seq TO pr_serving",
    )
    .await;
    let mut serving = url::Url::parse(
        &std::env::var("JC_PORTAL_TEST_DATABASE_URL").expect("JC_PORTAL_TEST_DATABASE_URL"),
    )
    .expect("url");
    serving.set_username("pr_serving").expect("user");
    serving.set_password(Some("pr-serving")).expect("password");
    let pool = sqlx::PgPool::connect(serving.as_str())
        .await
        .expect("the serving role connects");
    let store = ConnectorStore::new(Some((pool.clone(), std::sync::Arc::clone(&mirror))), None);
    let base = "https://portal.test";

    let made = scope(host(&a), store.create(&owner, ask(), vec![], base))
        .await
        .expect("created in a");
    let call: Call = serde_json::from_value(json!({
        "at": "2026-11-19T00:00:00Z", "subject": owner, "client": null, "tool": "query",
        "target": null, "verdict": "allowed", "durationMs": 3
    }))
    .expect("a call");
    scope(host(&a), store.record(&made.id, call))
        .await
        .expect("recorded");
    assert_eq!(
        scope(host(&a), store.calls(&made.id))
            .await
            .expect("calls")
            .len(),
        1
    );
    // On b's host the connector does not exist: no read, revoke, edit or call reaches it.
    assert!(scope(host(&b), store.get(&made.id, base))
        .await
        .expect("get")
        .is_none());
    assert!(scope(host(&b), store.of_owner(&owner, base))
        .await
        .expect("list")
        .is_empty());
    assert!(scope(host(&b), store.calls(&made.id))
        .await
        .expect("calls")
        .is_empty());
    assert!(!scope(host(&b), store.revoke(&made.id, &owner))
        .await
        .expect("revoke"));
    assert_eq!(
        scope(host(&b), store.revoke_all(&owner))
            .await
            .expect("revoke all"),
        0
    );
    assert!(scope(host(&b), store.overview(base))
        .await
        .expect("overview")
        .is_empty());
    // The same name in b is b's own connector.
    scope(host(&b), store.create(&owner, ask(), vec![], base))
        .await
        .expect("the same name in b");
    // Off any Organization host, with no installation Organization, nothing is read or kept.
    assert!(store.of_owner(&owner, base).await.expect("list").is_empty());
    assert!(matches!(
        store.create(&owner, ask(), vec![], base).await,
        Err(joinedcontext_portal::error::ApiError::Conflict(_))
    ));
    // The configured installation Organization places the deployment host's requests.
    let installed = ConnectorStore::new(
        Some((pool.clone(), std::sync::Arc::clone(&mirror))),
        Some(a.clone()),
    );
    let mine = installed.of_owner(&owner, base).await.expect("list");
    assert_eq!(
        mine.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        [made.id.as_str()]
    );

    // Preferences: one document per Organization, an upsert in b never meets a's.
    let save = |org: String, theme: &'static str| {
        let (pool, mirror, owner) = (pool.clone(), std::sync::Arc::clone(&mirror), owner.clone());
        scope(host(&org), async move {
            let mut tx = project_ids::person_tx(&pool, &mirror, None)
                .await
                .expect("tx")
                .expect("keyed");
            joinedcontext_portal::db::save_preferences(&mut tx, &owner, &json!({ "theme": theme }))
                .await
                .expect("saved");
            tx.commit().await.expect("commit");
        })
    };
    save(a.clone(), "dark").await;
    save(b.clone(), "light").await;
    let load = |org: String| {
        let (pool, mirror, owner) = (pool.clone(), std::sync::Arc::clone(&mirror), owner.clone());
        scope(host(&org), async move {
            let mut tx = project_ids::person_tx(&pool, &mirror, None)
                .await
                .expect("tx")
                .expect("keyed");
            joinedcontext_portal::db::load_preferences(&mut tx, &owner)
                .await
                .expect("loaded")
        })
    };
    assert_eq!(load(a.clone()).await, Some(json!({ "theme": "dark" })));
    assert_eq!(load(b.clone()).await, Some(json!({ "theme": "light" })));
    assert!(project_ids::person_tx(&pool, &mirror, None)
        .await
        .expect("tx")
        .is_none());

    // The policy, as the serving role sees it: the key unset reads no row of the three tables.
    sqlx::raw_sql(
        "DO $$ BEGIN CREATE ROLE pr_rls_reader NOLOGIN NOBYPASSRLS; \
         EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$; \
         GRANT SELECT, INSERT, UPDATE, DELETE ON connectors, connector_calls, user_preferences \
         TO pr_rls_reader",
    )
    .execute(&admin)
    .await
    .expect("the reader role");
    let a_key = project_ids::reserved(&pool, &a)
        .await
        .expect("reserved")
        .expect("a key");
    for key in [None, Some(a_key)] {
        let mut tx = match key {
            Some(key) => joinedcontext_portal::db::tenant_tx(&admin, key)
                .await
                .expect("keyed"),
            None => admin.begin().await.expect("begin"),
        };
        sqlx::query("SET LOCAL ROLE pr_rls_reader")
            .execute(&mut *tx)
            .await
            .expect("role");
        let seen: (i64, i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM connectors WHERE owner = $1), \
                    (SELECT count(*) FROM connector_calls WHERE subject = $1), \
                    (SELECT count(*) FROM user_preferences WHERE subject = $1)",
        )
        .bind(&owner)
        .fetch_one(&mut *tx)
        .await
        .expect("counts");
        let expected = if key.is_some() { (1, 1, 1) } else { (0, 0, 0) };
        assert_eq!(seen, expected, "rows seen under {key:?}");
        if key.is_none() {
            assert!(
                sqlx::query(
                    "INSERT INTO user_preferences (subject, preferences) VALUES ($1, '{}')"
                )
                .bind(&owner)
                .execute(&mut *tx)
                .await
                .is_err(),
                "an unkeyed write is refused"
            );
        }
    }
}

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

/// T-7196 phase one and T-7201 phase two, on a database of the release before 0080: a row written
/// before the key is keyed by the backfill once the mirror places its project; a project the
/// mirror lacks, or whose Organization is not of the registry, keeps no key and is named; a row
/// written through the store carries the key by the column default; the forcing migration refuses
/// while a row has no key and names its project, then forces the policy, which lets each key read
/// only its own rows.
#[tokio::test]
async fn project_rows_are_keyed_by_the_backfill_and_the_policy_reads_only_their_key() {
    use joinedcontext_portal::grafana::{self, Kind, Visualization};
    let pool = migrated_before(80, "pkbf").await;
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

    let waiting = project_ids::backfill_tables(&pool, &state.mirror, &["grafana_visualizations"])
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

    // T-7201: the forcing migration waits for the project the registry holds with no key, and
    // names it; a name the registry never minted (jc-dev's `platform` activity row) can never be
    // keyed, so its row is set aside whole instead of stopping every start.
    let refused = migrate_on(&pool)
        .await
        .expect_err("refused while a row has no key")
        .to_string();
    assert!(refused.contains(orphan.as_str()), "{refused}");
    assert!(!refused.contains(gone.as_str()), "{refused}");
    assert!(refused.contains("Organizations page"), "{refused}");
    sqlx::query("DELETE FROM grafana_visualizations WHERE project = $1")
        .bind(&orphan)
        .execute(&pool)
        .await
        .expect("the unplaced project's rows removed");
    migrate_on(&pool).await.expect("forced");
    let set_aside: Vec<(String, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT source_table, project, content->>'id' FROM project_rows_set_aside")
            .fetch_all(&pool)
            .await
            .expect("set aside");
    assert_eq!(
        set_aside,
        vec![(
            "grafana_visualizations".to_owned(),
            Some(gone.clone()),
            Some(removed.clone())
        )],
        "only the row no project can ever key is set aside, whole"
    );

    // The forced table: the key alone decides what a role without BYPASSRLS reads.
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

/// T-7201 (jc-dev 2026-11-18): an activity row of `platform`, a name no Project ever had, stopped
/// every start at 0080, with no operator step to place it. 0080 sets such rows aside whole, with the
/// rows ON DELETE CASCADE would take with them (a comment's notification), and forces the rest.
#[tokio::test]
async fn rows_no_project_can_key_are_set_aside_and_the_key_forced() {
    let pool = migrated_before(80, "pkaside").await;
    let gone = run_name("asidegone");
    sqlx::query(
        "INSERT INTO activity (time, project, kind, source, summary, severity) \
         VALUES (now(), 'platform', 'mcp.tool', 'portal', 'anna read through Claude (MCP)', 'info')",
    )
    .execute(&pool)
    .await
    .expect("the platform row");
    let comment: i64 = sqlx::query_scalar(
        "INSERT INTO entity_comments (project, space, urn, author, author_name, body) \
         VALUES ($1, 'air', 'urn:ngsi-ld:Device:1', 'anna', 'Anna', 'calibrated') RETURNING id",
    )
    .bind(&gone)
    .fetch_one(&pool)
    .await
    .expect("a comment");
    sqlx::query(
        "INSERT INTO notifications (recipient, project, space, urn, comment_id, author, author_name, excerpt) \
         VALUES ('eeva', $1, 'air', 'urn:ngsi-ld:Device:1', $2, 'anna', 'Anna', 'calibrated')",
    )
    .bind(&gone)
    .bind(comment)
    .execute(&pool)
    .await
    .expect("its notification");

    migrate_on(&pool)
        .await
        .expect("0080 runs: no row waits for an operator step");

    let mut set_aside: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT source_table, project FROM project_rows_set_aside")
            .fetch_all(&pool)
            .await
            .expect("set aside");
    set_aside.sort();
    assert_eq!(
        set_aside,
        vec![
            ("activity".to_owned(), Some("platform".to_owned())),
            ("entity_comments".to_owned(), Some(gone.clone())),
            ("notifications".to_owned(), Some(gone.clone())),
        ]
    );
    let left: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM activity) + (SELECT count(*) FROM entity_comments) \
         + (SELECT count(*) FROM notifications)",
    )
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(left, 0);
    let forced: bool =
        sqlx::query_scalar("SELECT relforcerowsecurity FROM pg_class WHERE relname = 'activity'")
            .fetch_one(&pool)
            .await
            .expect("forced");
    assert!(forced);
}

/// T-7264 (jc-prod 2026-11-18): a release-56 database gets 0057-0080 in one start, so no release
/// ran the backfill on it and 0080 refused its 5299 activity rows and 3 drafts. The start that is
/// told the installation Organization keys every project's rows under it before 0080, the
/// Organization's own rows (`org`) under its reserved project, a project named like another
/// Organization included, and 0080 sets aside nothing.
#[tokio::test]
async fn a_release_56_database_keys_every_project_under_the_installation_in_one_start() {
    let (pool, url) = migrated_before_at(57, "pk56").await;
    for (project, kind) in [
        ("bratislava", "config.applied"),
        ("bratislava", "pipeline.run"),
        ("bbsk", "config.applied"),
        ("org", "person.invited"),
    ] {
        sqlx::query(
            "INSERT INTO activity (time, project, kind, source, summary, severity) \
             VALUES (now(), $1, $2, 'portal', 'Applied.', 'info')",
        )
        .bind(project)
        .bind(kind)
        .execute(&pool)
        .await
        .expect("a release-56 activity row");
    }
    for project in ["bratislava", "bbsk", "org"] {
        sqlx::query(
            "INSERT INTO drafts (project, kind, name, manifest, touched_by, touched_kind) \
             VALUES ($1, 'Endpoint', 'air', '{}', 'anna', 'human')",
        )
        .bind(project)
        .execute(&pool)
        .await
        .expect("a release-56 draft");
    }
    pool.close().await;

    let smartcity = joinedcontext_portal::host_registry::SeedOrganization {
        name: "smartcity".to_owned(),
        organization_id: None,
        host: "portal.smartcity.example".to_owned(),
        catalogue_id: None,
    };
    let pool = joinedcontext_portal::db::connect_with(&url, Some(&smartcity))
        .await
        .expect("0057-0080 in one start: every row keyed before 0080");

    let unkeyed: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM activity WHERE org_id IS NULL OR project_id IS NULL) \
         + (SELECT count(*) FROM drafts WHERE org_id IS NULL OR project_id IS NULL) \
         + (SELECT count(*) FROM project_rows_set_aside)",
    )
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(unkeyed, 0, "every row keyed, none set aside");
    let rows: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM activity) + (SELECT count(*) FROM drafts)",
    )
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(rows, 7, "no row lost");
    let organizations: Vec<String> =
        sqlx::query_scalar("SELECT name FROM organizations ORDER BY name")
            .fetch_all(&pool)
            .await
            .expect("registry");
    assert_eq!(
        organizations,
        ["smartcity"],
        "no Organization made from a project name"
    );
    let foreign: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM (SELECT org_id FROM activity UNION ALL SELECT org_id FROM drafts) r \
         WHERE r.org_id <> (SELECT key FROM organizations WHERE name = 'smartcity')",
    )
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(foreign, 0, "every row under the installation Organization");
    let placed: Vec<(String, Option<String>, bool)> = sqlx::query_as(
        "SELECT DISTINCT a.project, p.organization, p.reserved FROM activity a \
         JOIN project_ids p ON p.id = a.project_id ORDER BY 1",
    )
    .fetch_all(&pool)
    .await
    .expect("placed");
    assert_eq!(
        placed,
        vec![
            ("bbsk".to_owned(), Some("smartcity".to_owned()), false),
            ("bratislava".to_owned(), Some("smartcity".to_owned()), false),
            ("org".to_owned(), Some("smartcity".to_owned()), true),
        ]
    );
    let forced: bool =
        sqlx::query_scalar("SELECT relforcerowsecurity FROM pg_class WHERE relname = 'activity'")
            .fetch_one(&pool)
            .await
            .expect("forced");
    assert!(forced);
}

/// T-7264: a start that stopped between 0079 and 0080 leaves the registry holding the
/// installation Organization alone; the next start keys the rows as the first would have, while a
/// database whose registry holds another Organization keeps the backfill's way (0080 decides).
#[tokio::test]
async fn a_start_after_one_stopped_before_0080_keys_the_rows_again() {
    let smartcity = joinedcontext_portal::host_registry::SeedOrganization {
        name: "smartcity".to_owned(),
        organization_id: None,
        host: "portal.smartcity.example".to_owned(),
        catalogue_id: None,
    };
    for (other, keyed) in [(None, true), (Some("bbsk"), false)] {
        let (pool, url) = migrated_before_at(80, "pkresume").await;
        joinedcontext_portal::host_registry::seed_organizations(
            &pool,
            std::slice::from_ref(&smartcity),
        )
        .await
        .expect("seeded before the stop");
        if let Some(other) = other {
            sqlx::query(
                "INSERT INTO organizations (name, host, state, created_by) \
                 VALUES ($1, $1 || '.example', 'active', 'test')",
            )
            .bind(other)
            .execute(&pool)
            .await
            .expect("another Organization");
        }
        sqlx::query(
            "INSERT INTO activity (time, project, kind, source, summary, severity) \
             VALUES (now(), 'bratislava', 'config.applied', 'portal', 'Applied.', 'info')",
        )
        .execute(&pool)
        .await
        .expect("an unkeyed row");
        project_ids::ensure(&pool, &["bratislava".to_owned()])
            .await
            .expect("its id, minted with no Organization yet");
        pool.close().await;
        let started = joinedcontext_portal::db::connect_with(&url, Some(&smartcity)).await;
        if keyed {
            let pool = started.expect("keyed, then 0080");
            let unkeyed: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM activity WHERE org_id IS NULL OR project_id IS NULL",
            )
            .fetch_one(&pool)
            .await
            .expect("count");
            assert_eq!(unkeyed, 0);
        } else {
            let err = started
                .expect_err("0080 refuses: the registry was in use")
                .to_string();
            assert!(err.contains("project rows without a key"), "{err}");
        }
    }
}

/// T-7282: a database of 0086 holds people's connectors, calls and preferences with no
/// Organization (jc-dev: 35, 637 and 8). A start told the installation Organization keys each
/// under its reserved project before 0088 forces the key, a call under its connector's, and loses
/// none; a start without it refuses at 0088 and names the setting.
#[tokio::test]
async fn a_persons_rows_of_0086_are_keyed_under_the_installation_before_0088() {
    for told in [true, false] {
        let (pool, url) = migrated_before_at(87, if told { "pr86" } else { "pr86no" }).await;
        sqlx::raw_sql(
            "INSERT INTO connectors (id, owner, name, grant_doc) VALUES ('c1', 'f:1:ana', 'doprava', '{}'); \
             INSERT INTO connector_calls (connector, subject, tool, verdict, duration_ms) \
             VALUES ('c1', 'f:1:ana', 'query', 'allowed', 3); \
             INSERT INTO user_preferences (subject, preferences) VALUES ('f:1:ana', '{\"theme\":\"dark\"}')",
        )
        .execute(&pool)
        .await
        .expect("a person's rows of 0086");
        pool.close().await;
        let marek = joinedcontext_portal::host_registry::SeedOrganization {
            name: "marek-mraz".to_owned(),
            organization_id: None,
            host: "portal.marek-mraz.example".to_owned(),
            catalogue_id: None,
        };
        let started = joinedcontext_portal::db::connect_with(&url, told.then_some(&marek)).await;
        if !told {
            let err = started.expect_err("0088 refuses unkeyed rows").to_string();
            assert!(err.contains("JC_PORTAL_INSTALLATION_ORGANIZATION"), "{err}");
            continue;
        }
        let pool = started.expect("keyed, then 0088");
        let key = project_ids::reserved(&pool, "marek-mraz")
            .await
            .expect("reserved")
            .expect("the installation's key");
        let keys: Vec<(Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT org_id::text, project_id::text FROM connectors \
             UNION ALL SELECT org_id::text, project_id::text FROM connector_calls \
             UNION ALL SELECT org_id::text, project_id::text FROM user_preferences",
        )
        .fetch_all(&pool)
        .await
        .expect("keys");
        let want = (Some(key.org.to_string()), Some(key.project.to_string()));
        assert_eq!(
            keys,
            vec![want.clone(), want.clone(), want],
            "every row keyed, none lost"
        );
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

/// A fresh database migrated up to `before` (excluded), as an installation running the release
/// before that migration holds it; dropped by nothing: each run's name is its own.
async fn migrated_before(before: i64, test: &str) -> sqlx::PgPool {
    migrated_before_at(before, test).await.0
}

/// [`migrated_before`] with the fresh database's URL, for a start that migrates it the way the
/// Portal does.
async fn migrated_before_at(before: i64, test: &str) -> (sqlx::PgPool, String) {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL").expect("JC_PORTAL_TEST_DATABASE_URL");
    let name = format!("{test}_{}", ProjectId::mint().path_segment());
    let admin = sqlx::PgPool::connect(&url)
        .await
        .expect("the test database");
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .execute(&admin)
        .await
        .expect("a fresh database");
    admin.close().await;
    let mut own = url::Url::parse(&url).expect("url");
    own.set_path(&format!("/{name}"));
    let pool = sqlx::PgPool::connect(own.as_str()).await.expect("fresh");
    let mut migrator = sqlx::migrate!("./migrations");
    migrator.migrations = migrator
        .migrations
        .iter()
        .filter(|m| m.version < before)
        .cloned()
        .collect::<Vec<_>>()
        .into();
    migrator.run(&pool).await.expect("migrated");
    (pool, own.to_string())
}

/// Applies every migration this release ships, the forcing migration 0080 (T-7201) included.
/// Without the migrator's advisory lock: a refused run returns its connection to the pool still
/// holding it, and the test's next run would wait for it forever.
async fn migrate_on(pool: &sqlx::PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    let mut migrator = sqlx::migrate!("./migrations");
    migrator.set_locking(false);
    migrator.run(pool).await
}

/// Rows of `rows` (table, insert binding `$1` to the project) for a placed and an unplaced
/// project of this run: the backfill keys the placed one's and names the other as waiting, and
/// under the policy the forcing migration turns on (rolled back here) each table reads the placed
/// project's row under its key only. Answers the state, the two projects and the key.
async fn rows_keyed_and_read_only_under_their_key(
    rows: &'static [(&'static str, &'static str)],
    test: &str,
) -> (AppState, String, String, jc_types::ProjectKey) {
    let pool = migrated_before(80, &format!("pk{test}")).await;
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
    let tables: Vec<&str> = rows.iter().map(|(t, _)| *t).collect();
    // Tables still on the list are phase one (0074's, T-7203): 0080 neither checks nor forces them.
    let phase_one = tables
        .iter()
        .all(|t| project_ids::KEYED_BY_NAME.contains(t));
    assert!(
        phase_one
            || tables
                .iter()
                .all(|t| !project_ids::KEYED_BY_NAME.contains(t)),
        "{tables:?}: all forced by 0080 or all phase one"
    );

    let waiting = project_ids::backfill_tables(&pool, &state.mirror, &tables)
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

    if phase_one {
        // 0080 runs over the orphan's unkeyed rows and leaves these tables as they are.
        migrate_on(&pool)
            .await
            .expect("0080 ignores phase-one rows");
        rls_reads_only_under_the_key(&pool, &org, &tables, &placed, key, true).await;
        return (state, placed, orphan, key);
    }

    // T-7201: the forcing migration refuses while the orphan's rows have no key, naming each
    // table and project and the next step; once they are gone it forces the key.
    let refused = migrate_on(&pool)
        .await
        .expect_err("refused while a row has no key")
        .to_string();
    for table in &tables {
        assert!(
            refused.contains(&format!("{table} (projects ")),
            "{table}: {refused}"
        );
    }
    assert!(refused.contains(&orphan), "{refused}");
    assert!(!refused.contains(&placed), "{refused}");
    for table in &tables {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM {table} WHERE {} = $1",
            project_ids::key_column(table)
        )))
        .bind(&orphan)
        .execute(&pool)
        .await
        .expect("the orphan's rows removed");
    }
    migrate_on(&pool)
        .await
        .expect("forced once every row has a key");
    for table in &tables {
        let shape: (i64, bool) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM information_schema.columns WHERE table_schema = 'public' \
                     AND table_name = $1 AND column_name IN ('org_id', 'project_id') \
                     AND is_nullable = 'NO'), \
                    (SELECT relrowsecurity AND relforcerowsecurity FROM pg_class \
                     WHERE oid = to_regclass('public.' || $1))",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .expect("catalog");
        assert_eq!(
            shape,
            (2, true),
            "{table}: both key columns NOT NULL, RLS forced"
        );
    }

    rls_reads_only_under_the_key(&pool, &org, &tables, &placed, key, false).await;
    (state, placed, orphan, key)
}

/// Under forced row-level security (turned on in each rolled-back transaction when `force`, for a
/// phase-one table), a role without BYPASSRLS reads `placed`'s row of each table under its key
/// only: none under another key of the Organization, none unkeyed.
async fn rls_reads_only_under_the_key(
    pool: &sqlx::PgPool,
    org: &str,
    tables: &[&str],
    placed: &str,
    key: jc_types::ProjectKey,
    force: bool,
) {
    let other = project_ids::reserved(pool, org)
        .await
        .expect("reserved")
        .expect("a key");
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "DO $$ BEGIN CREATE ROLE pk_rls_reader NOLOGIN NOBYPASSRLS; \
         EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$; \
         GRANT SELECT ON {} TO pk_rls_reader",
        tables.join(", ")
    )))
    .execute(pool)
    .await
    .expect("the reader role");
    for table in tables {
        for (key, expected) in [(Some(key), 1_i64), (Some(other), 0), (None, 0)] {
            let mut tx = pool.begin().await.expect("begin");
            if force {
                sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                    "ALTER TABLE {table} ENABLE ROW LEVEL SECURITY; \
                     ALTER TABLE {table} FORCE ROW LEVEL SECURITY"
                )))
                .execute(&mut *tx)
                .await
                .expect("forced in this transaction");
            }
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
            .bind(placed)
            .fetch_one(&mut *tx)
            .await
            .expect("count");
            assert_eq!(n, expected, "{table} {key:?}");
            tx.rollback().await.expect("rollback");
        }
    }
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
/// store writes a new draft in the key. T-7201: once 0080 forced the key, a project with no key
/// (its Organization not an active one of the registry) keeps no draft: the put is refused with
/// the step that fixes it and writes no row, and once the Organization is added it keeps drafts.
#[tokio::test]
async fn drafts_are_keyed_and_read_only_under_their_key() {
    use joinedcontext_portal::ops::drafts::DraftError;
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
    let refused = state
        .drafts
        .put(
            &orphan,
            "Endpoint",
            "water",
            manifest.clone(),
            None,
            "anna",
            "human",
        )
        .await
        .expect_err("a project with no key keeps no draft once 0080 forced the key");
    assert!(
        matches!(refused, DraftError::Unplaced(ref p) if p == &orphan),
        "{refused}"
    );
    assert!(
        refused.to_string().contains("Organizations page first"),
        "{refused}"
    );
    let stamped: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT project, project_id::text FROM drafts WHERE name = 'water' AND project = ANY($1)",
    )
    .bind(vec![placed.clone(), orphan.clone()])
    .fetch_all(&pool)
    .await
    .expect("rows");
    assert_eq!(
        stamped,
        [(placed.clone(), Some(key.project.to_string()))],
        "no row for the refused project"
    );
    assert!(state.drafts.list(&orphan).await.expect("list").is_empty());
    assert!(state
        .drafts
        .get(&orphan, "Endpoint", "water")
        .await
        .expect("get")
        .is_none());
    assert!(!state
        .drafts
        .drop(&orphan, "Endpoint", "water")
        .await
        .expect("drop"));
    let later = placed_later(&state, &orphan, "drlate").await;
    state
        .drafts
        .put(
            &orphan, "Endpoint", "water", manifest, None, "anna", "human",
        )
        .await
        .expect("kept once its Organization is added");
    let keyed: Vec<Option<String>> =
        sqlx::query_scalar("SELECT project_id::text FROM drafts WHERE project = $1")
            .bind(&orphan)
            .fetch_all(&pool)
            .await
            .expect("rows");
    assert_eq!(keyed, vec![Some(later.project.to_string())]);
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
/// new event lands in its project's key. T-7201: once 0080 forced the key, a project with no key
/// keeps no event: the batch stores every other project's events, names the refused project, and
/// the tail hears nothing of it; once its Organization is added its events are kept in its key.
#[tokio::test]
async fn activity_rows_are_keyed_and_read_only_under_their_key() {
    use joinedcontext_portal::activity::{ActivityError, ActivityEvent, ActivityFilter};
    let (state, placed, orphan, key) =
        rows_keyed_and_read_only_under_their_key(ACTIVITY_ROWS_0077, "ac").await;
    let pool = state.db.clone().expect("a database");
    let event = |project: &str| ActivityEvent {
        time: jc_time::TimestampTz::now(),
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
    let refused = state
        .activity
        .append(&[event(&orphan), event(&placed)])
        .await
        .expect_err("a project with no key keeps no event");
    assert!(
        matches!(refused, ActivityError::Unplaced(ref p) if p == &orphan),
        "{refused}"
    );
    assert!(tail.try_recv().is_err(), "the tail hears no refused event");
    let stamped: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT project, project_id::text FROM activity WHERE summary = 'Applied again.' \
         AND project = ANY($1)",
    )
    .bind(vec![placed.clone(), orphan.clone()])
    .fetch_all(&pool)
    .await
    .expect("rows");
    assert_eq!(
        stamped,
        [(placed.clone(), Some(key.project.to_string()))],
        "the placed project's event is kept, the refused one's is not"
    );
    let filter = ActivityFilter {
        limit: 10,
        ..ActivityFilter::default()
    };
    let unkeyed = state.activity.list(&orphan, &filter).await.expect("list");
    assert!(unkeyed.items.is_empty(), "a project with no key reads none");
    let page = state.activity.list(&placed, &filter).await.expect("list");
    assert_eq!(page.items.len(), 2, "the backfilled event and the new one");
    let later = placed_later(&state, &orphan, "aclate").await;
    state
        .activity
        .append(&[event(&orphan)])
        .await
        .expect("kept once its Organization is added");
    assert!(tail.try_recv().is_ok(), "the tail hears the kept event");
    let keyed: Vec<Option<String>> =
        sqlx::query_scalar("SELECT project_id::text FROM activity WHERE project = $1")
            .bind(&orphan)
            .fetch_all(&pool)
            .await
            .expect("rows");
    assert_eq!(keyed, vec![Some(later.project.to_string())]);

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
        0,
        "the orphan's unkeyed request was removed before the forcing migration"
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
    for (project, expected) in [(&placed, 1), (&orphan, 0)] {
        assert_eq!(
            open.iter().filter(|e| &e.project == project).count(),
            expected,
            "{project}: the placed one's erasure is open, read once; the orphan's was removed \
             before the forcing migration: {open:?}"
        );
    }
}

/// A row of each table T-7203 keys for `project`, written as before the key. `$1` is the project.
const PROJECT_ROWS_0074: &[(&str, &str)] = &[
    (
        "notifications",
        "INSERT INTO notifications (recipient, kind, project, space, urn, author, author_name, excerpt, event_key) \
         VALUES ('pj-' || md5($1), 'connector.pending', $1, '', 'c-1', 'sami', 'Sami', 'doprava', 'k-' || md5($1))",
    ),
    (
        "app_builds",
        "INSERT INTO app_builds (project, app, commit, class, trigger, state) \
         VALUES ($1, 'bikes', 'c0ffee', 'node', 'push', 'succeeded')",
    ),
];

/// T-7203 PK-1j: the notifications and App builds of today are keyed by the backfill and read
/// only under their key; an installation's connector event (project '') takes the installation
/// Organization's reserved project key; the inbox and the build queue read every key, and a
/// build is reached only with its own project.
#[tokio::test]
async fn notification_and_build_rows_are_keyed_and_read_only_under_their_key() {
    use joinedcontext_portal::apps::build_pods::Class;
    use joinedcontext_portal::apps::build_records::{self, Trigger};
    use joinedcontext_portal::entity_comments::{CommentStore, Notice};

    let (state, placed, orphan, key) =
        rows_keyed_and_read_only_under_their_key(PROJECT_ROWS_0074, "pj").await;
    let pool = state.db.clone().expect("a database");
    let org = project_ids::recorded(&pool, &placed)
        .await
        .expect("recorded")
        .map(|recorded| recorded.org);
    assert_eq!(org, Some(key.org));
    let org_name: String =
        sqlx::query_scalar("SELECT organization FROM project_ids WHERE id = $1::uuid")
            .bind(key.project.to_string())
            .fetch_one(&pool)
            .await
            .expect("the placed project's Organization");
    state.mirror.upsert(envelope(
        "Organization",
        &org_name,
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({ "installation": true }),
    ));
    let installation = project_ids::reserved(&pool, &org_name)
        .await
        .expect("reserved")
        .expect("the installation's reserved key");
    assert_eq!(
        project_ids::installation(&pool, &state.mirror)
            .await
            .expect("installation"),
        Some(installation)
    );

    // An installation's event written before the key is keyed by the backfill under the
    // installation's reserved project.
    let early = run_name("pjearly");
    sqlx::query(
        "INSERT INTO notifications (recipient, kind, project, space, urn, author, author_name, excerpt, event_key) \
         VALUES ($1, 'connector.expiring', '', '', 'c-1', '', '', 'doprava', 'early')",
    )
    .bind(&early)
    .execute(&pool)
    .await
    .expect("an unkeyed installation event");
    project_ids::backfill(&pool, &state.mirror)
        .await
        .expect("backfill");
    let early_key: Option<String> =
        sqlx::query_scalar("SELECT project_id::text FROM notifications WHERE recipient = $1")
            .bind(&early)
            .fetch_one(&pool)
            .await
            .expect("row");
    assert_eq!(early_key, Some(installation.project.to_string()));

    // New notices take their project's key, an installation's the reserved one; the inbox reads
    // both, the unkeyed one of the orphan project too, and marks one read in its key.
    let comments = CommentStore::new(Some(pool.clone()), state.mirror.clone());
    let me = run_name("pjme");
    fn notice<'a>(project: &'a str, key: &'a str) -> Notice<'a> {
        Notice {
            kind: "connector.pending",
            project,
            about: "c-1",
            author: "sami",
            author_name: "Sami",
            connector: "doprava",
            link: "/ai-connectors",
            key,
        }
    }
    for (project, event) in [(placed.as_str(), "e1"), ("", "e2"), (orphan.as_str(), "e3")] {
        assert!(comments
            .notify(&me, notice(project, event))
            .await
            .expect("notify"));
    }
    assert!(
        !comments
            .notify(&me, notice("", "e1"))
            .await
            .expect("notify"),
        "one event, one notification, across keys"
    );
    let stamped: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT event_key, project_id::text FROM notifications WHERE recipient = $1 ORDER BY event_key",
    )
    .bind(&me)
    .fetch_all(&pool)
    .await
    .expect("rows");
    assert_eq!(
        stamped,
        vec![
            ("e1".to_owned(), Some(key.project.to_string())),
            ("e2".to_owned(), Some(installation.project.to_string())),
            ("e3".to_owned(), None),
        ]
    );
    let (listed, unread) = comments
        .notifications(std::slice::from_ref(&me))
        .await
        .expect("inbox");
    assert_eq!((listed.len(), unread), (3, 3), "{listed:?}");
    assert!(listed.windows(2).all(|w| w[0].id > w[1].id), "newest first");
    let installation_notice = listed.iter().find(|n| n.project.is_empty()).expect("e2");
    assert!(comments
        .mark_read(std::slice::from_ref(&me), installation_notice.id)
        .await
        .expect("mark"));
    assert_eq!(
        comments
            .notifications(std::slice::from_ref(&me))
            .await
            .expect("inbox")
            .1,
        2
    );
    // An audience's notice is copied under its own key.
    let audience = run_name("pjaudience");
    let reader = run_name("pjreader");
    assert!(comments
        .notify(&audience, notice(&placed, "e4"))
        .await
        .expect("notify"));
    comments
        .deliver(&reader, &[], std::slice::from_ref(&audience))
        .await
        .expect("deliver");
    let copied: Option<String> =
        sqlx::query_scalar("SELECT project_id::text FROM notifications WHERE recipient = $1")
            .bind(&reader)
            .fetch_one(&pool)
            .await
            .expect("the copy");
    assert_eq!(copied, Some(key.project.to_string()));

    // A build is queued in its project's key, found by id across keys, and reached only with
    // its own project: a token of another project's build never reads this one.
    let app = run_name("pjapp");
    let id = build_records::enqueue(&pool, &placed, &app, "c1", Class::Node, Trigger::Push, None)
        .await
        .expect("queued");
    let build_key: Option<String> =
        sqlx::query_scalar("SELECT project_id::text FROM app_builds WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("row");
    assert_eq!(build_key, Some(key.project.to_string()));
    let found = build_records::get(&pool, id)
        .await
        .expect("get")
        .expect("found across keys");
    assert_eq!(
        (found.project.as_str(), found.app.as_str()),
        (placed.as_str(), app.as_str())
    );
    assert!(build_records::in_state(&pool, "pending")
        .await
        .expect("in_state")
        .iter()
        .any(|build| build.id == id));
    assert!(
        !build_records::start(&pool, &orphan, id, "t", jiff::SignedDuration::from_mins(5))
            .await
            .expect("start")
    );
    assert!(
        build_records::start(&pool, &placed, id, "t", jiff::SignedDuration::from_mins(5))
            .await
            .expect("start")
    );
    assert_eq!(
        build_records::token_of(&pool, &orphan, id)
            .await
            .expect("token"),
        None
    );
    assert_eq!(
        build_records::token_of(&pool, &placed, id)
            .await
            .expect("token"),
        Some(build_records::token_hash("t"))
    );
    build_records::finish(&pool, &placed, id, Err("ended for this test"))
        .await
        .expect("finished");
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
/// key and keeps none for a project without one.
#[tokio::test]
async fn pipeline_rows_are_keyed_and_read_only_under_their_key() {
    let (state, placed, orphan, key) =
        rows_keyed_and_read_only_under_their_key(PIPELINE_ROWS_0072, "pl").await;
    let pool = state.db.clone().expect("a database");
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
        "SELECT project, project_id::text FROM pipeline_log WHERE run = 'r2' AND project = ANY($1)",
    )
    .bind(vec![placed.clone(), orphan.clone()])
    .fetch_all(&pool)
    .await
    .expect("rows");
    assert_eq!(
        stamped,
        [(placed.clone(), Some(key.project.to_string()))],
        "a placed project's line lands in its key and an orphan's line is not kept"
    );
    assert_eq!(
        state
            .pipeline_log
            .runs(&orphan, "trees", 10)
            .await
            .expect("runs")
            .len(),
        0
    );
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
    let ran = state.pipeline_log.ran().await.expect("ran");
    assert!(ran.contains_key(&(placed.clone(), "trees".to_owned())));
    assert!(
        !ran.contains_key(&(orphan.clone(), "trees".to_owned())),
        "the pass over every project leaves a project with no key out (0080 forces the key)"
    );
    let refused = state
        .pipeline_templates
        .create(
            &orphan,
            "alice",
            joinedcontext_portal::ops::pipeline_templates::CreateTemplate {
                title: "Trees".into(),
                manifest: serde_json::json!({"kind": "Pipeline", "spec": {"class": "batch"}}),
            },
        )
        .await
        .expect_err("a project with no key keeps no template");
    assert!(
        refused.to_string().contains(&orphan) && refused.to_string().contains("Organizations page"),
        "the refusal names the project and the operator's next step: {refused}"
    );
}
