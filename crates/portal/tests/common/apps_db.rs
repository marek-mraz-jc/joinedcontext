//! The apps database of the wasm Apps against a real PostgreSQL 16 (AP-149, AP-150): a fresh
//! database owned by the Portal's CREATEROLE login, as on the cluster, never a superuser.
//!
//! `JC_APPS_DB_TEST_URL` names a server whose user may create roles and databases, e.g.
//! `docker run -e POSTGRES_PASSWORD=pw -p 5434:5432 postgres:16` and
//! `postgres://postgres:pw@127.0.0.1:5434/postgres`. A missing variable fails, never skips.

use joinedcontext_portal::apps::apps_db::AppsDb;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{ConnectOptions, PgPool};

pub const ADMIN: &str = "apps_admin_test";

pub fn server() -> PgConnectOptions {
    std::env::var("JC_APPS_DB_TEST_URL")
        .expect(
            "set JC_APPS_DB_TEST_URL to a PostgreSQL 16 whose user may create roles and databases",
        )
        .parse()
        .expect("JC_APPS_DB_TEST_URL parses")
}

/// Runs `statement` on the server, a role or database another test made at once being no failure.
pub async fn ensure(pool: &PgPool, statement: &str) {
    let guarded = format!(
        "DO $$ BEGIN {statement}; EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$"
    );
    sqlx::query(sqlx::AssertSqlSafe(guarded))
        .execute(pool)
        .await
        .expect(statement);
}

pub struct World {
    pub db: AppsDb,
    pub superuser: PgPool,
    pub host: PgConnectOptions,
    /// Unique to this run: roles are the server's, not the database's, so an App of an earlier
    /// run's database would otherwise share this run's App roles.
    pub run: String,
}

impl World {
    /// A project name of this run alone.
    pub fn project(&self, name: &str) -> String {
        format!("{name}-{}", self.run)
    }
}

/// A fresh database owned by the Portal's CREATEROLE login, with the shards' login roles.
pub async fn world(test: &str) -> World {
    world_of(test, 2).await
}

/// The superuser's statements of apps-db's bootstrap (apps_db/superuser.sql), as CloudNativePG
/// runs them in deployment components/apps-db.
pub const SUPERUSER_SQL: &str = include_str!("../../apps_db/superuser.sql");

pub async fn world_of(test: &str, shards: u32) -> World {
    let root = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(server())
        .await
        .expect("the test server answers");
    ensure(
        &root,
        &format!("CREATE ROLE {ADMIN} LOGIN CREATEROLE PASSWORD 'pw'"),
    )
    .await;
    for shard in 0..2 {
        ensure(
            &root,
            &format!("CREATE ROLE wasm_host_{shard} LOGIN NOINHERIT PASSWORD 'pw'"),
        )
        .await;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let name = format!("apps_{test}_{nanos}");
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE DATABASE {name} OWNER {ADMIN}"
    )))
    .execute(&root)
    .await
    .expect("create the database");
    let here = server().database(&name);
    let superuser = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(here.clone())
        .await
        .expect("superuser");
    // jc_set_config is the server's, made once; the revokes are this database's own.
    let statements: Vec<&str> = SUPERUSER_SQL
        .lines()
        .filter(|line| !line.trim_start().starts_with("--"))
        .collect::<Vec<_>>()
        .join("\n")
        .leak()
        .split(';')
        .map(str::trim)
        .filter(|statement| !statement.is_empty())
        .collect();
    for statement in statements {
        if statement.starts_with("CREATE ROLE") {
            ensure(&superuser, statement).await;
        } else {
            sqlx::query(sqlx::AssertSqlSafe(statement.to_owned()))
                .execute(&superuser)
                .await
                .expect(statement);
        }
    }
    // As deployment components/apps-db makes them members (inRoles), inheriting.
    for member in [ADMIN, "wasm_host_0", "wasm_host_1"] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "GRANT jc_set_config TO {member} WITH INHERIT TRUE"
        )))
        .execute(&superuser)
        .await
        .expect("member of jc_set_config");
    }
    let admin_options = here.clone().username(ADMIN).password("pw");
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(admin_options.clone())
        .await
        .expect("the Portal's login");
    let db = AppsDb {
        admin,
        connect: here.clone().disable_statement_logging(),
        shards,
    };
    db.bootstrap().await.expect("jc_apps applies");
    World {
        db,
        superuser,
        host: here,
        run: nanos.to_string(),
    }
}
