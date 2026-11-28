//! The Postgres settings of one module (`[<module>.database]` in the config file) and the
//! connection they open.
//!
//! The URL names role, host, port and database and never a password: the password is read from
//! `password_file` (the secrets mount). With `migrate_url` (and its own password file) `jc` runs
//! the module's migrations first, as that owning role, under the module's advisory lock; without
//! it the schema must already be migrated, and the module's boot check refuses an unmigrated
//! one. A module with a `database` section never falls back to its memory store: a failed
//! connection or a failed boot RLS check stops `jc`.

use serde::Deserialize;
use sqlx::postgres::{PgConnectOptions, PgPool};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

/// Connections a module's pool opens at most when `max_connections` is unset.
pub const DEFAULT_MAX_CONNECTIONS: u32 = 5;

/// `[<module>.database]`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseSection {
    /// `postgres://<request role>@<host>:<port>/<database>`, without a password.
    pub url: String,
    /// File holding the request role's password.
    pub password_file: PathBuf,
    /// Pool size (default [`DEFAULT_MAX_CONNECTIONS`]).
    pub max_connections: Option<u32>,
    /// `postgres://<migration role>@...`: when set, `jc` migrates at start as this role.
    pub migrate_url: Option<String>,
    /// File holding the migration role's password (required with `migrate_url`).
    pub migrate_password_file: Option<PathBuf>,
    /// Antares only: `postgres://<maintenance role>@...`, the member of `antares_maintenance` the
    /// broker runs its cross-Tenant work as (T-7310).
    #[serde(default)]
    pub maintenance_url: Option<String>,
    /// File holding the maintenance role's password (required with `maintenance_url`).
    #[serde(default)]
    pub maintenance_password_file: Option<PathBuf>,
}

impl DatabaseSection {
    /// Checks the section without touching the network or the files: both URLs parse, carry a
    /// role and no password, and the migration pair is complete.
    pub fn validate(&self) -> Result<(), String> {
        check_url("url", &self.url)?;
        match (&self.migrate_url, &self.migrate_password_file) {
            (Some(url), Some(_)) => check_url("migrate_url", url).map(drop),
            (None, None) => Ok(()),
            _ => Err("migrate_url and migrate_password_file go together".into()),
        }?;
        match (&self.maintenance_url, &self.maintenance_password_file) {
            (Some(url), Some(_)) => check_url("maintenance_url", url).map(drop),
            (None, None) => Ok(()),
            _ => Err("maintenance_url and maintenance_password_file go together".into()),
        }?;
        if self.max_connections == Some(0) {
            return Err("max_connections must be at least 1".into());
        }
        Ok(())
    }

    pub fn max_connections(&self) -> u32 {
        self.max_connections.unwrap_or(DEFAULT_MAX_CONNECTIONS)
    }

    /// Options of the request role, password read from its file.
    pub fn app_options(&self) -> Result<PgConnectOptions, String> {
        options(&self.url, &self.password_file)
    }

    /// Options of the migration role, when one is configured.
    pub fn migrate_options(&self) -> Result<Option<PgConnectOptions>, String> {
        match (&self.migrate_url, &self.migrate_password_file) {
            (Some(url), Some(file)) => options(url, file).map(Some),
            _ => Ok(None),
        }
    }

    /// The request role's URL with its password, for a module that takes a URL string
    /// (Antares). Hold it as a secret: never log or print it.
    pub fn app_url_with_password(&self) -> Result<jc_secret::Secret, String> {
        url_with_password("url", &self.url, &self.password_file)
    }

    /// The migration role's URL with its password, when one is configured (Antares).
    pub fn migrate_url_with_password(&self) -> Result<Option<jc_secret::Secret>, String> {
        match (&self.migrate_url, &self.migrate_password_file) {
            (Some(url), Some(file)) => url_with_password("migrate_url", url, file).map(Some),
            _ => Ok(None),
        }
    }

    /// The maintenance role's URL with its password, when one is configured (Antares).
    pub fn maintenance_url_with_password(&self) -> Result<Option<jc_secret::Secret>, String> {
        match (&self.maintenance_url, &self.maintenance_password_file) {
            (Some(url), Some(file)) => url_with_password("maintenance_url", url, file).map(Some),
            _ => Ok(None),
        }
    }

    /// The role `url` names (the role a migration grants to).
    pub fn app_role(&self) -> Result<String, String> {
        role_of("url", &self.url)
    }

    /// The owner/request split a database serving requests under row-level security needs
    /// (T-7488): `migrate_url` names the owner, `url` a login that owns nothing, and with
    /// `maintenance` (Antares) `maintenance_url` a third login. One login in two places would
    /// serve requests as the owner, which can switch its tables' row-level security off.
    pub fn require_owner_split(&self, maintenance: bool) -> Result<(), String> {
        let Some(owner_url) = &self.migrate_url else {
            return Err(
                "migrate_url is required: jc migrates as the owner of the tables \
                 (migrate_url) and serves as a login that owns nothing (url)"
                    .into(),
            );
        };
        let request = self.app_role()?;
        let owner = role_of("migrate_url", owner_url)?;
        if request == owner {
            return Err(format!(
                "url and migrate_url both name `{owner}`: url must name a login that owns \
                 nothing, e.g. `{owner}_app`"
            ));
        }
        if !maintenance {
            return Ok(());
        }
        let Some(maint_url) = &self.maintenance_url else {
            return Err(
                "maintenance_url is required: Antares runs its cross-Tenant work as a \
                 member of antares_maintenance, apart from the request login"
                    .into(),
            );
        };
        let maint = role_of("maintenance_url", maint_url)?;
        if maint == request || maint == owner {
            return Err(format!(
                "maintenance_url names `{maint}`, which url or migrate_url names too: give it a \
                 login of its own, e.g. `{owner}_maint`"
            ));
        }
        Ok(())
    }
}

fn role_of(key: &str, raw: &str) -> Result<String, String> {
    Ok(PgConnectOptions::from_str(raw)
        .map_err(|_| format!("{key} does not parse"))?
        .get_username()
        .to_owned())
}

/// The boot refusal of every request pool that row-level security guards (Antares, the Portal,
/// the assistant; T-7488): the pool's login must not be a superuser, have BYPASSRLS, belong to
/// `owner` (the migrating login) or own a relation outside the system schemas. Each would let a
/// request path read or write another tenant's rows, or switch row-level security off. `key`
/// names the setting that configured the login, so the error says what to change.
pub async fn refuse_unsafe_request_role(
    key: &str,
    pool: &PgPool,
    owner: &str,
) -> Result<(), String> {
    let (database, login, superuser, bypass, member, owned): (
        String,
        String,
        bool,
        bool,
        bool,
        Vec<String>,
    ) = sqlx::query_as(
        "SELECT current_database()::text, current_user::text, r.rolsuper, r.rolbypassrls, \
           EXISTS (SELECT 1 FROM pg_catalog.pg_roles o WHERE o.rolname = $1 \
                     AND pg_catalog.pg_has_role(current_user, o.oid, 'MEMBER')), \
           ARRAY(SELECT n.nspname || '.' || c.relname FROM pg_catalog.pg_class c \
                   JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
                  WHERE n.nspname <> 'information_schema' AND n.nspname NOT LIKE 'pg\\_%' \
                    AND c.relkind IN ('r', 'p', 'v', 'm', 'S') \
                    AND pg_catalog.pg_has_role(current_user, c.relowner, 'MEMBER') \
                  ORDER BY 1 LIMIT 5) \
         FROM pg_catalog.pg_roles r WHERE r.rolname = current_user",
    )
    .bind(owner)
    .fetch_one(pool)
    .await
    .map_err(|e| format!("{key}: the request login check failed: {e}"))?;
    let why = if superuser {
        "is a superuser".to_owned()
    } else if bypass {
        "has BYPASSRLS".to_owned()
    } else if member {
        format!("is a member of the owner `{owner}`")
    } else if !owned.is_empty() {
        format!("owns {}", owned.join(", "))
    } else {
        return Ok(());
    };
    Err(format!(
        "database {database}: the request login `{login}` ({key}) {why}, so a request could pass \
         row-level security or switch it off; serve as a login with NOBYPASSRLS that owns \
         nothing and belongs to no owner, and migrate as `{owner}`"
    ))
}

/// `raw` with the password of `file` in it. Hold it as a secret: never log or print it.
fn url_with_password(key: &str, raw: &str, file: &Path) -> Result<jc_secret::Secret, String> {
    let mut url = check_url(key, raw)?;
    let password = read_password(file)?;
    url.set_password(Some(password.expose()))
        .map_err(|()| format!("{key} takes no password"))?;
    Ok(jc_secret::Secret::new(url.into()))
}

fn check_url(key: &str, raw: &str) -> Result<url::Url, String> {
    let url = url::Url::parse(raw).map_err(|_| format!("{key} is not a URL"))?;
    if !matches!(url.scheme(), "postgres" | "postgresql") {
        return Err(format!("{key} must use the postgres:// scheme"));
    }
    if url.password().is_some() {
        return Err(format!(
            "{key} must not carry a password; put it in the password file"
        ));
    }
    if url.username().is_empty() {
        return Err(format!("{key} must name the database role"));
    }
    // The driver also takes the role and the password from the query: a `password` there is a
    // secret in the config file, a `user` a login other than the one checked here.
    if let Some((name, _)) = url
        .query_pairs()
        .find(|(name, _)| matches!(name.as_ref(), "password" | "user"))
    {
        return Err(format!(
            "{key} must not set `{name}` in its query: name the role before the @ and put the \
             password in the password file"
        ));
    }
    PgConnectOptions::from_str(raw).map_err(|_| format!("{key} is not a Postgres URL"))?;
    Ok(url)
}

/// The password in `path`, through `jc`'s one secret reader: trimmed, an empty file refused, the
/// value never in an error.
fn read_password(path: &Path) -> Result<jc_secret::Secret, String> {
    jc_secret::read_file("password_file", path).map_err(|e| match e {
        jc_secret::ReadError::File { reason, .. } => {
            format!("cannot read password file {}: {reason}", path.display())
        }
        jc_secret::ReadError::EmptyFile { .. } => {
            format!("password file {} is empty", path.display())
        }
        other => other.to_string(),
    })
}

fn options(url: &str, password_file: &Path) -> Result<PgConnectOptions, String> {
    let opts = PgConnectOptions::from_str(url).map_err(|_| "url does not parse".to_owned())?;
    Ok(opts.password(read_password(password_file)?.expose()))
}

/// A pool of at most `max` connections; a refused or unreachable server is an error (no retry
/// beyond the acquire timeout, no fallback).
pub async fn connect(opts: PgConnectOptions, max: u32) -> Result<PgPool, String> {
    jc_pool::options::<sqlx::Postgres>()
        .max_connections(max.max(1))
        .acquire_timeout(Duration::from_secs(10))
        .connect_with(opts)
        .await
        .map_err(|e| format!("cannot connect to Postgres: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(url: &str) -> DatabaseSection {
        DatabaseSection {
            url: url.into(),
            password_file: "/run/pw".into(),
            max_connections: None,
            migrate_url: None,
            migrate_password_file: None,
            maintenance_url: None,
            maintenance_password_file: None,
        }
    }

    /// A role needs a host: `postgres://role@/db` is no URL, so every URL that names its role
    /// takes the password (the `set_password` refusal cannot arise); the password is never in
    /// an error.
    #[test]
    fn a_url_without_a_host_is_refused_and_one_with_a_host_takes_the_password() {
        let dir = tempfile::tempdir().expect("a scratch dir");
        let file = dir.path().join("pw");
        std::fs::write(&file, "s3cret-pw\n").expect("the password file");
        let e = url_with_password("url", "postgres://shako@/jc", &file)
            .err()
            .expect("no host");
        assert_eq!(e, "url is not a URL");
        let ok = url_with_password("url", "postgres://shako@db/jc", &file).expect("a host");
        assert!(ok.expose().contains(":s3cret-pw@db"));
        let missing = url_with_password("url", "postgres://shako@db/jc", &dir.path().join("none"))
            .err()
            .expect("no file");
        assert!(
            missing.starts_with("cannot read password file"),
            "{missing}"
        );
    }

    #[test]
    fn a_url_with_a_password_or_without_a_role_is_refused() {
        assert!(section("postgres://shako@db:5432/jc").validate().is_ok());
        for (bad, says) in [
            ("postgres://shako:leaked@db/jc", "must not carry a password"),
            ("postgres://db/jc", "must name the database role"),
            ("mysql://shako@db/jc", "postgres:// scheme"),
            ("not a url", "is not a URL"),
            // Found by the pg_section fuzz target: the driver reads both from the query.
            (
                "postgres://shako@db/jc?password=leaked",
                "must not set `password`",
            ),
            (
                "postgres://shako@db/jc?sslmode=disable&user=postgres",
                "must not set `user`",
            ),
            ("postgres://shako@db/jc?user=", "must not set `user`"),
        ] {
            let err = section(bad).validate().unwrap_err();
            assert!(err.contains(says), "{bad}: {err}");
            assert!(!err.contains("leaked"), "{err}");
        }
        let mut s = section("postgres://shako@db/jc");
        s.migrate_url = Some("postgres://shako_migrate@db/jc".into());
        assert!(s.validate().unwrap_err().contains("go together"));
        s.migrate_password_file = Some("/run/mpw".into());
        assert!(s.validate().is_ok());
        s.migrate_url = Some("postgres://shako_migrate:x@db/jc".into());
        assert!(s.validate().unwrap_err().starts_with("migrate_url"));
        s.migrate_url = None;
        s.migrate_password_file = None;
        s.max_connections = Some(0);
        assert!(s.validate().is_err());
    }

    #[test]
    fn the_maintenance_pair_goes_together_and_parses() {
        let mut s = section("postgres://antares_app@db/jc");
        s.maintenance_url = Some("postgres://antares_maint@db/jc".into());
        assert!(s.validate().unwrap_err().contains("maintenance_url and"));
        s.maintenance_password_file = Some("/run/maint".into());
        assert!(s.validate().is_ok());
        s.maintenance_url = Some("postgres://antares_maint:x@db/jc".into());
        assert!(s.validate().unwrap_err().starts_with("maintenance_url"));
        s.maintenance_url = None;
        assert!(s.validate().unwrap_err().contains("go together"));
        s.maintenance_password_file = None;
        s.max_connections = Some(12);
        assert_eq!(s.max_connections(), 12);
    }

    #[test]
    fn the_owner_split_needs_three_distinct_logins() {
        let mut s = section("postgres://antares@db/jc");
        assert!(
            s.require_owner_split(false)
                .unwrap_err()
                .contains("migrate_url is required")
        );
        s.migrate_url = Some("postgres://antares@db/jc".into());
        let e = s.require_owner_split(false).unwrap_err();
        assert!(
            e.contains("both name `antares`") && e.contains("antares_app"),
            "{e}"
        );
        s.url = "postgres://antares_app@db/jc".into();
        assert!(s.require_owner_split(false).is_ok());
        assert!(
            s.require_owner_split(true)
                .unwrap_err()
                .contains("maintenance_url is required")
        );
        for same in ["antares_app", "antares"] {
            s.maintenance_url = Some(format!("postgres://{same}@db/jc"));
            let e = s.require_owner_split(true).unwrap_err();
            assert!(
                e.contains(&format!("names `{same}`")) && e.contains("antares_maint"),
                "{e}"
            );
        }
        s.maintenance_url = Some("postgres://antares_maint@db/jc".into());
        assert!(s.require_owner_split(true).is_ok());
        // A URL that is no Postgres URL names its key.
        s.maintenance_url = Some("postgres://antares_maint@db:notaport/jc".into());
        assert_eq!(
            s.require_owner_split(true).unwrap_err(),
            "maintenance_url does not parse"
        );
        s.migrate_url = Some("postgres://antares@db:notaport/jc".into());
        assert_eq!(
            s.require_owner_split(false).unwrap_err(),
            "migrate_url does not parse"
        );
        s.url = "postgres://antares_app@db:notaport/jc".into();
        assert_eq!(s.app_role().unwrap_err(), "url does not parse");
    }

    #[test]
    fn each_role_url_takes_its_own_password_file() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, value: &str| {
            let path = dir.path().join(name);
            std::fs::write(&path, value).unwrap();
            path
        };
        let mut s = section("postgres://antares_app@db/jc");
        s.password_file = write("app", "a");
        assert!(s.migrate_url_with_password().unwrap().is_none());
        assert!(s.maintenance_url_with_password().unwrap().is_none());
        s.migrate_url = Some("postgres://antares@db/jc".into());
        s.migrate_password_file = Some(write("owner", "o p/q\n"));
        s.maintenance_url = Some("postgresql://antares_maint@db:5433/jc".into());
        s.maintenance_password_file = Some(write("maint", "m\n"));
        // A password with URL delimiters is percent-encoded, never splits the URL.
        assert_eq!(
            s.migrate_url_with_password().unwrap().unwrap().expose(),
            "postgres://antares:o%20p%2Fq@db/jc"
        );
        assert_eq!(
            s.maintenance_url_with_password().unwrap().unwrap().expose(),
            "postgresql://antares_maint:m@db:5433/jc"
        );
        assert!(s.migrate_options().unwrap().is_some());
        // A missing migration password file is an error naming the file, not a silent None.
        s.migrate_password_file = Some(dir.path().join("gone"));
        assert!(s.migrate_options().unwrap_err().contains("gone"));
        assert!(
            s.migrate_url_with_password()
                .unwrap_err()
                .contains("cannot read")
        );
        // A URL check runs before the file is read.
        s.url = "postgres://antares_app:leak@db/jc".into();
        let e = s.app_url_with_password().unwrap_err();
        assert!(e.starts_with("url must not carry a password"), "{e}");
        s.url = "postgres://antares_app@db:notaport/jc".into();
        assert!(s.app_options().unwrap_err().contains("does not parse"));
        // A URL the URL parser takes and the Postgres driver does not.
        let e = section("postgres://antares_app@db/jc?sslmode=sometimes")
            .validate()
            .unwrap_err();
        assert_eq!(e, "url is not a Postgres URL");
    }

    /// T-7488 against a real server: each unsafe request login is refused with the reason and
    /// the setting to change; a login that owns nothing passes. Runs when
    /// `JC_PG_TEST_DATABASE_URL` names a superuser connection (as tests/restart_pg.rs).
    #[tokio::test]
    async fn every_unsafe_request_login_is_refused_with_its_reason() {
        let admin_url = crate::config::portal_env("JC_PG_TEST_DATABASE_URL")
            .expect("JC_PG_TEST_DATABASE_URL is set by ci.yml root-test");
        let admin = connect(PgConnectOptions::from_str(&admin_url).unwrap(), 1)
            .await
            .unwrap();
        let p = format!("pgrole{}", std::process::id());
        let pw = "pg-role-test-pw";
        for (role, how) in [
            ("owner", "NOLOGIN"),
            ("ok", "LOGIN NOBYPASSRLS"),
            ("bypass", "LOGIN BYPASSRLS"),
            ("member", "LOGIN NOBYPASSRLS"),
            ("owns", "LOGIN NOBYPASSRLS"),
        ] {
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "DROP ROLE IF EXISTS {p}_{role}"
            )))
            .execute(&admin)
            .await
            .unwrap();
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "CREATE ROLE {p}_{role} {how} PASSWORD '{pw}'"
            )))
            .execute(&admin)
            .await
            .unwrap();
        }
        for q in [
            format!("GRANT {p}_owner TO {p}_member"),
            format!("CREATE TABLE public.{p}_t (id int)"),
            format!("ALTER TABLE public.{p}_t OWNER TO {p}_owns"),
        ] {
            sqlx::raw_sql(sqlx::AssertSqlSafe(q))
                .execute(&admin)
                .await
                .unwrap();
        }
        let base = PgConnectOptions::from_str(&admin_url).unwrap();
        let check = |role: &str| {
            let opts = base.clone().username(&format!("{p}_{role}")).password(pw);
            let owner = format!("{p}_owner");
            async move {
                let pool = connect(opts, 1).await.unwrap();
                let got = refuse_unsafe_request_role("url", &pool, &owner).await;
                pool.close().await;
                got
            }
        };
        assert_eq!(check("ok").await, Ok(()));
        for (role, why) in [
            ("bypass", "has BYPASSRLS".to_owned()),
            ("member", format!("is a member of the owner `{p}_owner`")),
            ("owns", format!("owns public.{p}_t")),
        ] {
            let e = check(role).await.unwrap_err();
            assert!(e.contains(&format!("`{p}_{role}` (url) {why}")), "{e}");
            assert!(e.contains("migrate as") && !e.contains(pw), "{e}");
        }
        // The superuser itself.
        let e = refuse_unsafe_request_role("url", &admin, "nobody")
            .await
            .unwrap_err();
        assert!(e.contains("is a superuser"), "{e}");
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP TABLE public.{p}_t")))
            .execute(&admin)
            .await
            .unwrap();
        for role in ["owner", "ok", "bypass", "member", "owns"] {
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP ROLE {p}_{role}")))
                .execute(&admin)
                .await
                .unwrap();
        }
        // A closed pool: the check names the setting.
        admin.close().await;
        let e = refuse_unsafe_request_role("migrate_url", &admin, "x")
            .await
            .unwrap_err();
        assert!(
            e.starts_with("migrate_url: the request login check failed"),
            "{e}"
        );
    }

    #[tokio::test]
    async fn an_unreachable_server_is_an_error_naming_postgres() {
        // Bound and never listening: the port stays refused for the test's life.
        let closed = tokio::net::TcpSocket::new_v4().unwrap();
        closed.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let port = closed.local_addr().unwrap().port();
        let opts = PgConnectOptions::new()
            .host("127.0.0.1")
            .port(port)
            .username("nobody")
            .password("pw");
        // max 0 is raised to one connection, not a pool that never connects.
        let e = connect(opts, 0).await.unwrap_err();
        assert!(e.starts_with("cannot connect to Postgres"), "{e}");
        assert!(!e.contains("pw"), "{e}");
        drop(closed);
    }

    /// A URL whose role names no host has nowhere to put the password: refused, naming the key.
    #[test]
    fn a_url_without_a_host_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("pw");
        std::fs::write(&file, "pw-no-host").unwrap();
        let e = url_with_password("url", "postgres://jc@/jc", &file).unwrap_err();
        assert!(e.starts_with("url "), "{e}");
        assert!(!e.contains("pw-no-host"), "{e}");
    }

    #[test]
    fn the_password_comes_from_its_file_only() {
        let dir = tempfile::tempdir().unwrap();
        let pw = dir.path().join("pw");
        std::fs::write(&pw, "s3cret\n").unwrap();
        let mut s = section("postgres://antares@db:5432/jc");
        s.password_file = pw.clone();
        assert_eq!(
            s.app_url_with_password().unwrap().expose(),
            "postgres://antares:s3cret@db:5432/jc"
        );
        assert_eq!(s.app_role().unwrap(), "antares");
        assert!(s.app_options().is_ok());
        assert_eq!(s.max_connections(), DEFAULT_MAX_CONNECTIONS);
        std::fs::write(&pw, " \n").unwrap();
        assert!(s.app_options().unwrap_err().contains("is empty"));
        s.password_file = dir.path().join("missing");
        assert!(s.app_options().unwrap_err().contains("cannot read"));
        assert!(s.migrate_options().unwrap().is_none());
    }
}
