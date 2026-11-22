#![forbid(unsafe_code)]
//! Migration 0004 cleans the orphans of two realms as `kaisatsu_migrate`, under FORCE RLS, and its
//! revert restores them without granting what was created again since.

use kaisatsu_store_pg::PgStore;
use serde_json::{Value, json};
use sqlx::{Connection, PgPool};

mod common;

const REVERT: &str = include_str!("../operator/0004_revert.sql");
const REPORT: &str = include_str!("../operator/0004_report.sql");

/// The database as it was before 0004.
async fn before_0004(pool: &PgPool) {
    sqlx::raw_sql(
        "DROP TABLE kaisatsu.grant_cleanup_0004;
         DELETE FROM kaisatsu_migrations._sqlx_migrations WHERE version = 4;",
    )
    .execute(pool)
    .await
    .expect("back to version 3");
}

/// One realm as deletes without a cascade left it.
async fn seed(su: &PgPool, realm: &str) {
    let person = json!({
        "id": "ada", "username": "ada",
        "realmRoles": ["kept", "mapped", "deleted-role", "mfa-required", "manage-users"],
        "clientRoles": {
            "app-kept": ["editor", "deleted-editor", "mapped-editor"],
            "deleted-app": ["editor"],
            "realm-management": ["view-users"],
            "gone-realm": ["view-users"],
        },
        "groups": ["/team", "/nowhere"],
    });
    let team = json!({ "id": "team", "name": "team", "path": "/team",
        "realmRoles": ["kept", "deleted-role"] });
    let lost = json!({ "id": "lost", "name": "child", "path": "/deleted/child",
        "parentId": "deleted" });
    let moved = json!({ "id": "moved", "name": "sub", "path": "/team/sub",
        "parentId": "old-team" });
    let parent = json!({ "id": "kept", "name": "kept",
        "composites": { "realm": [], "client": { "deleted-app": ["editor"] } } });
    let app = json!({ "id": "app", "clientId": "app-kept",
        "attributes": { "managed-by": "joinedcontext" } });
    let docs = [person, team, lost, moved, parent, app];
    let statements = [
        "INSERT INTO kaisatsu.people (realm_id, id, username_lower, doc) VALUES ($1, 'ada', 'ada', $2)",
        "INSERT INTO kaisatsu.groups (realm_id, id, path, doc) VALUES ($1, 'team', '/team', $2)",
        "INSERT INTO kaisatsu.groups (realm_id, id, path, doc) VALUES ($1, 'lost', '/deleted/child', $2)",
        "INSERT INTO kaisatsu.groups (realm_id, id, path, doc) VALUES ($1, 'moved', '/team/sub', $2)",
        "INSERT INTO kaisatsu.roles (realm_id, id, name, doc) VALUES ($1, 'kept', 'kept', $2)",
        "INSERT INTO kaisatsu.applications (realm_id, id, client_id, doc) VALUES ($1, 'app', 'app-kept', $2)",
    ];
    sqlx::query("INSERT INTO kaisatsu.realms (realm_id, name, doc) VALUES ($1, $1, $2)")
        .bind(realm)
        .bind(json!({ "id": realm, "realm": realm }))
        .execute(su)
        .await
        .expect("realm");
    for (sql, doc) in statements.into_iter().zip(docs) {
        sqlx::query(sql)
            .bind(realm)
            .bind(doc)
            .execute(su)
            .await
            .expect("seed document");
    }
    for sql in [
        "INSERT INTO kaisatsu.roles (realm_id, id, container_id, client_role, name, doc)
         VALUES ($1, 'editor', 'app', true, 'editor', '{}'), ($1, 'stray', 'no-app', true, 'stray', '{}')",
        "INSERT INTO kaisatsu.group_members (realm_id, group_id, user_id, doc)
         VALUES ($1, 'lost', 'ada', '{}'), ($1, 'moved', 'ada', '{}'), ($1, 'team', 'no-person', '{}')",
        "INSERT INTO kaisatsu.role_mappings
             (realm_id, id, principal_kind, principal_id, client_uuid, role_name, doc)
         VALUES ($1, 'm1', 'user', 'ada', NULL, 'mapped', '{}'),
                ($1, 'm2', 'user', 'ada', 'app', 'mapped-editor', '{}'),
                ($1, 'm3', 'user', 'no-person', NULL, 'kept', '{}'),
                ($1, 'm4', 'group', 'team', 'no-app', 'editor', '{}')",
    ] {
        sqlx::query(sql).bind(realm).execute(su).await.expect("seed rows");
    }
}

async fn doc(su: &PgPool, table: &str, realm: &str, id: &str) -> Option<Value> {
    let sql = format!("SELECT doc FROM kaisatsu.{table} WHERE realm_id = $1 AND id = $2");
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(realm)
        .bind(id)
        .fetch_optional(su)
        .await
        .expect("document")
}

async fn ids(su: &PgPool, sql: &'static str, realm: &str) -> Vec<String> {
    sqlx::query_scalar(sql)
        .bind(realm)
        .fetch_all(su)
        .await
        .expect("ids")
}

#[tokio::test]
async fn migration_0004_cleans_orphans_in_every_realm_and_reverts() {
    let Some(db) = common::test_db("migration_0004").await else {
        return;
    };
    let su = db.superuser_pool().await;
    before_0004(&db.migrate_pool).await;
    for realm in ["one", "two"] {
        seed(&su, realm).await;
    }
    sqlx::raw_sql(REPORT)
        .execute(&db.migrate_pool)
        .await
        .expect("the report runs as the migration role");
    PgStore::run_migrations(&db.migrate_pool)
        .await
        .expect("0004");

    for realm in ["one", "two"] {
        let ada = doc(&su, "people", realm, "ada").await.expect("ada");
        assert_eq!(
            ada["realmRoles"],
            json!(["kept", "mapped", "mfa-required", "manage-users"])
        );
        assert_eq!(
            ada["clientRoles"],
            json!({
                "app-kept": ["editor", "mapped-editor"],
                "realm-management": ["view-users"],
                "gone-realm": ["view-users"],
            })
        );
        assert_eq!(ada["groups"], json!([]), "no path is backfilled or kept");
        let team = doc(&su, "groups", realm, "team").await.expect("team");
        assert_eq!(team["realmRoles"], json!(["kept"]));
        assert!(doc(&su, "groups", realm, "lost").await.is_none());
        let moved = doc(&su, "groups", realm, "moved").await.expect("moved");
        assert_eq!(moved["parentId"], "team");
        let parent = doc(&su, "roles", realm, "kept").await.expect("role");
        assert_eq!(parent["composites"]["client"], json!({}));
        assert!(doc(&su, "roles", realm, "stray").await.is_none());
        let members = ids(
            &su,
            "SELECT group_id::text FROM kaisatsu.group_members WHERE realm_id = $1",
            realm,
        )
        .await;
        assert_eq!(members, ["moved"]);
        let maps = ids(
            &su,
            "SELECT id::text FROM kaisatsu.role_mappings WHERE realm_id = $1 ORDER BY id",
            realm,
        )
        .await;
        assert_eq!(maps, ["m1", "m2"]);
    }

    let app = db.app_pool().await;
    assert!(
        sqlx::query("SELECT 1 FROM kaisatsu.grant_cleanup_0004")
            .execute(&app)
            .await
            .is_err(),
        "the request role reads what was removed"
    );
    app.close().await;

    // Created again in realm `two` since: its old grants must stay gone.
    for sql in [
        "INSERT INTO kaisatsu.roles (realm_id, id, name, doc) VALUES ('two', 'again', 'deleted-role', '{}')",
        "INSERT INTO kaisatsu.applications (realm_id, id, client_id, doc) VALUES ('two', 'new-app', 'deleted-app', '{}')",
    ] {
        sqlx::query(sql).execute(&su).await.expect("created again");
    }

    let mut conn = sqlx::PgConnection::connect_with(&db.migrate_options())
        .await
        .expect("migration role");
    assert!(
        sqlx::raw_sql(REVERT).execute(&mut conn).await.is_err(),
        "the revert runs only by a named choice"
    );
    sqlx::raw_sql("SET kaisatsu.restore = 'all'")
        .execute(&mut conn)
        .await
        .expect("choice");
    sqlx::raw_sql(REVERT)
        .execute(&mut conn)
        .await
        .expect("revert");
    conn.close().await.expect("close");

    let one = doc(&su, "people", "one", "ada").await.expect("ada");
    let has = |v: &Value, name: &str| {
        v.as_array()
            .is_some_and(|a| a.iter().any(|x| x.as_str() == Some(name)))
    };
    assert!(has(&one["realmRoles"], "deleted-role"));
    assert!(has(&one["clientRoles"]["app-kept"], "deleted-editor"));
    assert_eq!(one["clientRoles"]["deleted-app"], json!(["editor"]));
    assert_eq!(one["groups"], json!(["/team"]));
    assert!(doc(&su, "groups", "one", "lost").await.is_some());
    assert!(doc(&su, "roles", "one", "stray").await.is_some());

    let two = doc(&su, "people", "two", "ada").await.expect("ada");
    assert!(
        !has(&two["realmRoles"], "deleted-role"),
        "the revert widened a grant"
    );
    assert!(
        two["clientRoles"].get("deleted-app").is_none(),
        "the revert widened a grant"
    );

    PgStore::run_migrations(&db.migrate_pool)
        .await
        .expect("0004 applies again after its revert");
    su.close().await;
    db.drop_db().await;
}
