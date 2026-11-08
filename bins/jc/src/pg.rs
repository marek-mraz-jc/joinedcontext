//! PostgreSQL for the modules that keep state (`[<module>.database]` in the TOML file): the
//! application role's URL and password come from files, the pool runs the module's boot RLS
//! check, and when `migrate_url_file` is set the module's migrations run first as that role.
//! The application role must not own the tables or bypass RLS; each module's own boot check
//! refuses one that does.

use serde::Deserialize;
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

/// `[<module>.database]`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Database {
    /// File holding the application role's URL, without a password.
    pub url_file: PathBuf,
    /// File holding the application role's password.
    pub password_file: Option<PathBuf>,
    /// Pool size (default 5).
    pub max_connections: Option<u32>,
    /// File holding the migration role's URL, without a password; migrations run at start
    /// when set.
    pub migrate_url_file: Option<PathBuf>,
    /// File holding the migration role's password.
    pub migrate_password_file: Option<PathBuf>,
}

/// Connection options from a URL file and a password file. A URL that carries a password is
/// refused: the password belongs in its own file. Errors name files, never their content.
pub fn options(url_file: &Path, password_file: Option<&Path>) -> Result<PgConnectOptions, String> {
    let url = crate::runtime::read_secret_file(url_file)?;
    if carries_password(&url) {
        return Err(format!(
            "{}: the URL must not carry a password; put it in the password file",
            url_file.display()
        ));
    }
    let mut opts = PgConnectOptions::from_str(&url)
        .map_err(|_| format!("{}: not a PostgreSQL URL", url_file.display()))?;
    if let Some(file) = password_file {
        opts = opts.password(&crate::runtime::read_secret_file(file)?);
    }
    Ok(opts)
}

/// Whether a `postgres://` URL holds a password, in its user part or as a query parameter.
fn carries_password(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?']).next().unwrap_or_default();
    let in_userinfo = authority
        .rsplit_once('@')
        .is_some_and(|(userinfo, _)| userinfo.contains(':'));
    let in_query = rest.split_once('?').is_some_and(|(_, query)| {
        query
            .split('&')
            .any(|pair| pair.split('=').next() == Some("password"))
    });
    in_userinfo || in_query
}

async fn connect(opts: PgConnectOptions, max: u32) -> Result<PgPool, String> {
    PgPoolOptions::new()
        .max_connections(max.clamp(1, 50))
        .acquire_timeout(Duration::from_secs(10))
        .connect_with(opts)
        .await
        .map_err(|e| format!("connect PostgreSQL: {e}"))
}

impl Database {
    /// The application pool.
    pub async fn app_pool(&self) -> Result<PgPool, String> {
        let opts = options(&self.url_file, self.password_file.as_deref())?;
        connect(opts, self.max_connections.unwrap_or(5)).await
    }

    /// The migration pool, when migrations run at start.
    pub async fn migrate_pool(&self) -> Result<Option<PgPool>, String> {
        let Some(url_file) = &self.migrate_url_file else {
            return Ok(None);
        };
        let opts = options(url_file, self.migrate_password_file.as_deref())?;
        connect(opts, 1).await.map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_with_a_password_is_refused_and_one_without_is_read() {
        assert!(carries_password("postgres://app:pw@db:5432/x"));
        assert!(carries_password("postgres://db/x?user=app&password=pw"));
        assert!(!carries_password("postgres://app@db:5432/x"));
        assert!(!carries_password("postgres://db/x?sslmode=require"));
        assert!(!carries_password("postgres://app@[::1]:5432/x"));
        let dir = tempfile::tempdir().unwrap();
        let url = dir.path().join("url");
        let pw = dir.path().join("pw");
        std::fs::write(&url, "postgres://app:leaked@db/x\n").unwrap();
        std::fs::write(&pw, "s3cret\n").unwrap();
        let e = options(&url, Some(&pw)).unwrap_err();
        assert!(
            e.contains("must not carry a password") && !e.contains("leaked"),
            "{e}"
        );
        std::fs::write(&url, "postgres://app@db:5433/x\n").unwrap();
        let opts = options(&url, Some(&pw)).unwrap();
        assert_eq!(opts.get_port(), 5433);
        assert_eq!(opts.get_username(), "app");
    }
}

/// A fresh database per test on the server `JC_TEST_PG_ADMIN_URL` names (a superuser URL);
/// `None` with a SKIP line when it is unset. Migrations run as that superuser; the application
/// role is a new LOGIN role granted the module's `<module>_app` role, so forced RLS applies.
#[cfg(test)]
pub(crate) mod testdb {
    use super::Database;
    use sqlx::postgres::PgConnectOptions;
    use std::str::FromStr;

    pub(crate) struct TestDb {
        pub dir: tempfile::TempDir,
        pub database: Database,
        /// The superuser's address on the test database, for a bypassing-role check.
        pub admin: Database,
    }

    fn write(dir: &std::path::Path, name: &str, text: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        path
    }

    pub(crate) async fn create(module: &str) -> Option<TestDb> {
        let Ok(admin_url) = std::env::var("JC_TEST_PG_ADMIN_URL") else {
            eprintln!("SKIP: JC_TEST_PG_ADMIN_URL is unset");
            return None;
        };
        let opts = PgConnectOptions::from_str(&admin_url).unwrap();
        let admin = sqlx::PgPool::connect_with(opts.clone()).await.unwrap();
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let db = format!("jct_{module}_{}_{n}", std::process::id());
        let role = format!("jct_{module}_app");
        let password = format!("{n:x}{:x}", std::process::id());
        // Names and the password are made of [a-z0-9_] here; role DDL takes no binds.
        let ddl = format!(
            "DO $$ BEGIN
                IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{module}_app') THEN
                    CREATE ROLE {module}_app NOLOGIN NOBYPASSRLS;
                END IF;
                IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{module}_migrate') THEN
                    CREATE ROLE {module}_migrate NOLOGIN NOBYPASSRLS;
                END IF;
                IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{role}') THEN
                    CREATE ROLE {role} LOGIN NOBYPASSRLS;
                END IF;
             END $$;
             ALTER ROLE {role} LOGIN NOSUPERUSER NOBYPASSRLS PASSWORD '{password}';
             GRANT {module}_app TO {role};"
        );
        sqlx::raw_sql(sqlx::AssertSqlSafe(ddl))
            .execute(&admin)
            .await
            .unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE DATABASE {db}")))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
        let dir = tempfile::tempdir().unwrap();
        let host = opts.get_host();
        let port = opts.get_port();
        let su = opts.get_username();
        let admin_pw = write(
            dir.path(),
            "admin-pw",
            &url_password(&admin_url).unwrap_or_default(),
        );
        let admin_url_file = write(
            dir.path(),
            "admin-url",
            &format!("postgres://{su}@{host}:{port}/{db}"),
        );
        let database = Database {
            url_file: write(
                dir.path(),
                "app-url",
                &format!("postgres://{role}@{host}:{port}/{db}"),
            ),
            password_file: Some(write(dir.path(), "app-pw", &password)),
            max_connections: Some(2),
            migrate_url_file: Some(admin_url_file.clone()),
            migrate_password_file: Some(admin_pw.clone()),
        };
        let admin = Database {
            url_file: admin_url_file,
            password_file: Some(admin_pw),
            max_connections: Some(2),
            migrate_url_file: None,
            migrate_password_file: None,
        };
        Some(TestDb {
            dir,
            database,
            admin,
        })
    }

    /// The password of a URL's user part (taken as written: the test URL is not percent-encoded).
    fn url_password(url: &str) -> Option<String> {
        let rest = url.split_once("://")?.1;
        let userinfo = rest.split(['/', '?']).next()?.rsplit_once('@')?.0;
        Some(userinfo.split_once(':')?.1.to_owned())
    }
}
