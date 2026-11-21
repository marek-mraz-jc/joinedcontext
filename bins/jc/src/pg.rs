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
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
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
            (Some(url), Some(_)) => check_url("migrate_url", url),
            (None, None) => Ok(()),
            _ => Err("migrate_url and migrate_password_file go together".into()),
        }?;
        match (&self.maintenance_url, &self.maintenance_password_file) {
            (Some(url), Some(_)) => check_url("maintenance_url", url),
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
    check_url(key, raw)?;
    let password = read_password(file)?;
    let mut url = url::Url::parse(raw).map_err(|_| format!("{key} does not parse"))?;
    url.set_password(Some(password.expose()))
        .map_err(|()| format!("{key} takes no password"))?;
    Ok(jc_secret::Secret::new(url.into()))
}

fn check_url(key: &str, raw: &str) -> Result<(), String> {
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
    PgConnectOptions::from_str(raw).map_err(|_| format!("{key} is not a Postgres URL"))?;
    Ok(())
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
    PgPoolOptions::new()
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

    #[test]
    fn a_url_with_a_password_or_without_a_role_is_refused() {
        assert!(section("postgres://shako@db:5432/jc").validate().is_ok());
        for (bad, says) in [
            ("postgres://shako:leaked@db/jc", "must not carry a password"),
            ("postgres://db/jc", "must name the database role"),
            ("mysql://shako@db/jc", "postgres:// scheme"),
            ("not a url", "is not a URL"),
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
