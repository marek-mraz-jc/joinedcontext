//! Port of `internal/impl/sql/conn_fields.go` (Bento 1.22.0): the `driver` and `dsn` fields, the
//! connection settings every sql component shares, opening a pool, `init_files`/`init_statement`,
//! and `isAuthError`.
//!
//! Drivers: `postgres` (lib/pq DSNs), `mysql` (go-sql-driver DSNs) and `sqlite` (modernc DSNs)
//! run over sqlx. The other drivers Bento lists are refused at lint, naming the driver. A DSN's
//! host passes the organization's outbound rule before a pool is opened; a sqlite file stays
//! under the stream's file root.

use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use crate::sqlx::mysql::{MySqlConnectOptions, MySqlPool, MySqlPoolOptions, MySqlSslMode};
use crate::sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions, PgSslMode};
use ekiben_core::config::parse_duration_str;
use ekiben_core::{CoreError, OrgContext};
use serde_json::Value as JsonValue;
use tokio::sync::OnceCell;

/// Every driver Bento 1.22.0 accepts in `driver` (`driverNames`), in its order.
pub const DRIVER_NAMES: &[&str] = &[
    "mysql",
    "postgres",
    "clickhouse",
    "mssql",
    "sqlite",
    "oracle",
    "snowflake",
    "trino",
    "gocosmos",
    "spanner",
    "duckdb",
];

/// The connection fields of `connFields()`, shared by every sql component.
pub const CONN_FIELDS: &[&str] = &[
    "init_files",
    "init_statement",
    "init_verify_conn",
    "conn_max_idle_time",
    "conn_max_life_time",
    "conn_max_idle",
    "conn_max_open",
    "secret_name",
    "iam_enabled",
    "azure",
    // config.SessionFields()
    "region",
    "endpoint",
    "credentials",
];

/// `SQLConnLintRule`.
pub const SQL_CONN_LINT: &str = "both iam_enabled and secret_name cannot be set";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Driver {
    Postgres,
    MySql,
    Sqlite,
}

impl Driver {
    pub fn name(self) -> &'static str {
        match self {
            Driver::Postgres => "postgres",
            Driver::MySql => "mysql",
            Driver::Sqlite => "sqlite",
        }
    }

    /// The driver of `name`, or the lint error Bento (or ekiben) gives it.
    pub fn parse(name: &str) -> Result<Self, String> {
        match name {
            "postgres" => Ok(Driver::Postgres),
            "mysql" => Ok(Driver::MySql),
            "sqlite" => Ok(Driver::Sqlite),
            other if DRIVER_NAMES.contains(&other) => Err(format!(
                "driver {other} is not supported by ekiben (it runs postgres, mysql and sqlite)"
            )),
            other => Err(format!(
                "sql: unknown driver \"{other}\" (forgotten import?)"
            )),
        }
    }
}

/// The connection lint shared by every sql component: the driver, `SQLConnLintRule`, and the
/// cloud credential fields ekiben cannot honour.
pub fn lint_conn(obj: &serde_json::Map<String, JsonValue>) -> Vec<String> {
    let mut errs = Vec::new();
    match obj.get("driver") {
        // Bento does not lint the driver enum; a driver it lists that ekiben does not run is
        // refused here, a name it does not list fails at run as in Bento.
        Some(JsonValue::String(d)) if DRIVER_NAMES.contains(&d.as_str()) => {
            if let Err(e) = Driver::parse(d) {
                errs.push(e);
            }
        }
        Some(JsonValue::String(_)) => {}
        Some(_) => errs.push("field driver: expected string value".to_string()),
        None => {}
    }
    let iam = obj.get("iam_enabled").and_then(JsonValue::as_bool) == Some(true);
    if iam && obj.contains_key("secret_name") {
        errs.push(SQL_CONN_LINT.to_string());
    }
    for (field, why) in [
        (
            "secret_name",
            "AWS Secrets Manager credentials are not supported, give the DSN by a secretRef",
        ),
        ("region", "AWS sessions are not supported"),
        ("endpoint", "AWS sessions are not supported"),
        ("credentials", "AWS sessions are not supported"),
    ] {
        if obj.contains_key(field) {
            errs.push(format!("field {field} is not supported by ekiben: {why}"));
        }
    }
    if iam {
        errs.push(
            "field iam_enabled is not supported by ekiben: RDS IAM tokens are not supported"
                .to_string(),
        );
    }
    let entra = obj
        .get("azure")
        .and_then(|a| a.get("entra_enabled"))
        .and_then(JsonValue::as_bool)
        == Some(true);
    if entra {
        errs.push(
            "field azure.entra_enabled is not supported by ekiben: Azure Entra tokens are not supported"
                .to_string(),
        );
    }
    for key in ["conn_max_idle_time", "conn_max_life_time"] {
        if let Some(v) = obj.get(key) {
            if v.as_str()
                .map(parse_duration_str)
                .is_none_or(|r| r.is_err())
            {
                errs.push(format!("field {key}: expected a duration such as 30s"));
            }
        }
    }
    errs
}

/// `connSettings`: pool limits and the statements run once on the first connection.
pub struct ConnSettings {
    pub conn_max_lifetime: Option<Duration>,
    pub conn_max_idle_time: Option<Duration>,
    pub max_idle_conns: i64,
    pub max_open_conns: i64,
    pub init_file_statements: Vec<(String, String)>,
    pub init_statement: String,
    pub init_verify_conn: bool,
    init_once: OnceCell<()>,
}

fn duration_field(
    obj: &serde_json::Map<String, JsonValue>,
    key: &str,
) -> Result<Option<Duration>, CoreError> {
    match obj.get(key) {
        None => Ok(None),
        Some(JsonValue::String(s)) => parse_duration_str(s).map(Some),
        Some(_) => Err(CoreError::config(format!(
            "field {key}: expected a duration string"
        ))),
    }
}

fn int_field(
    obj: &serde_json::Map<String, JsonValue>,
    key: &str,
    default: i64,
) -> Result<i64, CoreError> {
    match obj.get(key) {
        None => Ok(default),
        Some(v) => v
            .as_i64()
            .ok_or_else(|| CoreError::config(format!("field {key}: expected an integer"))),
    }
}

/// Expands Bento's `init_files` globs (super globs included) under the stream's file root.
fn expand_init_files(
    patterns: &[String],
    root: Option<&Path>,
) -> Result<Vec<(String, String)>, CoreError> {
    let mut out = Vec::new();
    for pattern in patterns {
        let guarded = ekiben_io_file::path_guard::sanitize_file_path(pattern, root)?;
        let pat = guarded.to_string_lossy().to_string();
        let mut paths: Vec<String> = if pat.contains(['*', '?', '[']) {
            glob::glob(&pat)
                .map_err(|e| {
                    CoreError::config(format!("failed to expand init_files glob patterns: {e}"))
                })?
                .filter_map(Result::ok)
                .map(|p| p.to_string_lossy().to_string())
                .collect()
        } else {
            vec![pat]
        };
        paths.sort();
        for p in paths {
            let stmt = std::fs::read_to_string(&p)
                .map_err(|e| CoreError::config(format!("open {p}: {e}")))?;
            out.push((p, stmt));
        }
    }
    Ok(out)
}

impl ConnSettings {
    /// `connSettingsFromParsed`.
    pub fn from_config(
        obj: &serde_json::Map<String, JsonValue>,
        root: Option<&Path>,
    ) -> Result<Self, CoreError> {
        let init_files: Vec<String> = match obj.get("init_files") {
            None => Vec::new(),
            Some(JsonValue::Array(a)) => a
                .iter()
                .map(|v| {
                    v.as_str().map(String::from).ok_or_else(|| {
                        CoreError::config("field init_files: expected a list of strings")
                    })
                })
                .collect::<Result<_, _>>()?,
            Some(_) => {
                return Err(CoreError::config(
                    "field init_files: expected a list of strings",
                ))
            }
        };
        Ok(Self {
            conn_max_lifetime: duration_field(obj, "conn_max_life_time")?,
            conn_max_idle_time: duration_field(obj, "conn_max_idle_time")?,
            max_idle_conns: int_field(obj, "conn_max_idle", 2)?,
            max_open_conns: int_field(obj, "conn_max_open", 0)?,
            init_file_statements: expand_init_files(&init_files, root)?,
            init_statement: obj
                .get("init_statement")
                .and_then(JsonValue::as_str)
                .unwrap_or_default()
                .to_string(),
            init_verify_conn: obj
                .get("init_verify_conn")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false),
            init_once: OnceCell::new(),
        })
    }

    /// `apply`: runs `init_files` then `init_statement` once per component; a failure is a warning.
    pub async fn apply(&self, db: &Db) {
        self.init_once
            .get_or_init(|| async {
                for (path, stmt) in &self.init_file_statements {
                    match db.exec_raw(stmt).await {
                        Ok(()) => tracing::debug!("Successfully ran init_file '{path}'"),
                        Err(e) => tracing::warn!("Failed to execute init_file '{path}': {e}"),
                    }
                }
                if !self.init_statement.is_empty() {
                    match db.exec_raw(&self.init_statement).await {
                        Ok(()) => tracing::debug!("Successfully ran init_statement"),
                        Err(e) => tracing::warn!("Failed to execute init_statement: {e}"),
                    }
                }
            })
            .await;
    }

    fn pool_limits(&self) -> (u32, u32) {
        // database/sql: max_open <= 0 is unlimited, idle above max_open is cut to it.
        let max_open = if self.max_open_conns <= 0 {
            u32::MAX
        } else {
            u32::try_from(self.max_open_conns).unwrap_or(u32::MAX)
        };
        let idle = u32::try_from(self.max_idle_conns.max(0))
            .unwrap_or(0)
            .min(max_open);
        // ponytail: sqlx needs a finite pool; "unlimited" is 64 connections, raise with conn_max_open.
        (max_open.min(64), idle.min(max_open.min(64)))
    }
}

/// An open connection pool of one driver (`*sql.DB`).
#[derive(Clone)]
pub enum Db {
    Postgres(PgPool),
    MySql(MySqlPool),
    Sqlite(crate::sqlite::SqliteDb),
}

impl Db {
    pub fn driver(&self) -> Driver {
        match self {
            Db::Postgres(_) => Driver::Postgres,
            Db::MySql(_) => Driver::MySql,
            Db::Sqlite(_) => Driver::Sqlite,
        }
    }

    /// Runs `sql` without arguments as the Go drivers do (a simple query: several statements allowed).
    pub async fn exec_raw(&self, sql: &str) -> Result<(), crate::sqlx::Error> {
        let q = crate::sqlx::raw_sql(crate::sqlx::AssertSqlSafe(sql.to_string()));
        match self {
            Db::Postgres(p) => q.execute(p).await.map(drop),
            Db::MySql(p) => q.execute(p).await.map(drop),
            Db::Sqlite(d) => {
                let sql = sql.to_string();
                d.with(move |c| crate::sqlite::exec(c, &sql, &[]))
                    .await
                    .map_err(crate::sqlx::Error::Protocol)
            }
        }
    }

    pub async fn close(&self) {
        match self {
            Db::Postgres(p) => p.close().await,
            Db::MySql(p) => p.close().await,
            Db::Sqlite(d) => d.close(),
        }
    }
}

/// What a DSN connects to, before the outbound rule judges it.
pub enum Target {
    Postgres(PgConnectOptions),
    MySql(MySqlConnectOptions),
    Sqlite(crate::sqlite::SqliteTarget),
}

/// The redacted form of a DSN for errors: never the DSN itself, which may carry a password.
pub fn dsn_error(driver: Driver, why: impl std::fmt::Display) -> CoreError {
    CoreError::config(format!("invalid {} dsn: {why}", driver.name()))
}

/// Parses a lib/pq DSN, a URL (`postgres://u:p@host:5432/db?sslmode=disable`) or key=value
/// pairs (`host=h port=5432 user=u`), as lib/pq does: `sslmode` defaults to `require`, and a
/// parameter lib/pq does not know is a run-time parameter sent at startup.
pub fn parse_postgres_dsn(dsn: &str) -> Result<PgConnectOptions, CoreError> {
    let err = |why: &str| dsn_error(Driver::Postgres, why);
    let mut pairs: Vec<(String, String)> = Vec::new();
    if dsn.starts_with("postgres://") || dsn.starts_with("postgresql://") {
        let u = url::Url::parse(dsn).map_err(|_| err("not a postgres:// URL"))?;
        if let Some(h) = u.host_str() {
            pairs.push(("host".into(), h.trim_matches(['[', ']']).to_string()));
        }
        if let Some(p) = u.port() {
            pairs.push(("port".into(), p.to_string()));
        }
        if !u.username().is_empty() {
            let user = percent_decode(u.username());
            pairs.push(("user".into(), user));
        }
        if let Some(p) = u.password() {
            pairs.push(("password".into(), percent_decode(p)));
        }
        let db = u.path().trim_start_matches('/');
        if !db.is_empty() {
            pairs.push(("dbname".into(), percent_decode(db)));
        }
        for (k, v) in u.query_pairs() {
            pairs.push((k.into_owned(), v.into_owned()));
        }
    } else {
        pairs = parse_kv_dsn(dsn)
            .ok_or_else(|| err("expected key=value pairs or a postgres:// URL"))?;
    }
    let mut opts = PgConnectOptions::new_without_pgpass()
        .host("localhost")
        .port(5432)
        .password("")
        .ssl_mode(PgSslMode::Require);
    let mut user = None;
    let mut runtime: Vec<(String, String)> = Vec::new();
    for (k, v) in pairs {
        opts = match k.as_str() {
            "host" => {
                if v.starts_with('/') {
                    return Err(err("a unix socket host is not allowed"));
                }
                opts.host(&v)
            }
            "port" => opts.port(v.parse().map_err(|_| err("port is not a number"))?),
            "user" => {
                user = Some(v);
                opts
            }
            "password" => opts.password(&v),
            "dbname" => opts.database(&v),
            "sslmode" => {
                opts.ssl_mode(PgSslMode::from_str(&v).map_err(|_| err("unknown sslmode"))?)
            }
            "sslrootcert" | "sslcert" | "sslkey" | "sslinline" | "sslsni" | "sslpassword" => {
                return Err(dsn_error(
                    Driver::Postgres,
                    format!("parameter {k} is not supported by ekiben"),
                ))
            }
            "fallback_application_name" | "application_name" => opts.application_name(&v),
            "connect_timeout" => opts,
            "disable_prepared_binary_result" | "binary_parameters" | "krbsrvname" | "krbspn" => {
                opts
            }
            _ => {
                runtime.push((k, v));
                opts
            }
        };
    }
    opts = opts.username(user.as_deref().unwrap_or("postgres"));
    if !runtime.is_empty() {
        opts = opts.options(runtime);
    }
    Ok(opts)
}

fn percent_decode(s: &str) -> String {
    url::form_urlencoded::parse(format!("x={s}").as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

/// lib/pq's `key=value key2='quoted value'` DSN form.
fn parse_kv_dsn(dsn: &str) -> Option<Vec<(String, String)>> {
    let mut out = Vec::new();
    let mut chars = dsn.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            return Some(out);
        }
        let mut key = String::new();
        while let Some(&c) = chars.peek() {
            if c == '=' || c.is_whitespace() {
                break;
            }
            key.push(c);
            chars.next();
        }
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.next() != Some('=') || key.is_empty() {
            return None;
        }
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let mut val = String::new();
        if chars.peek() == Some(&'\'') {
            chars.next();
            loop {
                match chars.next()? {
                    '\\' => val.push(chars.next()?),
                    '\'' => break,
                    c => val.push(c),
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                if c == '\\' {
                    chars.next();
                    val.push(chars.next()?);
                } else {
                    val.push(c);
                    chars.next();
                }
            }
        }
        out.push((key, val));
    }
}

/// Parses a go-sql-driver DSN `[user[:password]@][net[(addr)]]/dbname[?param=value&...]`.
/// Returns the options and whether `parseTime=true`.
pub fn parse_mysql_dsn(dsn: &str) -> Result<(MySqlConnectOptions, bool), CoreError> {
    let err = |why: String| dsn_error(Driver::MySql, why);
    let slash = dsn
        .rfind('/')
        .ok_or_else(|| err("missing the slash before the database name".into()))?;
    let (head, tail) = dsn.split_at(slash);
    let tail = &tail[1..];
    let (dbname, params) = tail.split_once('?').unwrap_or((tail, ""));
    let (cred, netaddr) = match head.rfind('@') {
        Some(i) => (Some(&head[..i]), &head[i + 1..]),
        None => (None, head),
    };
    let mut opts = MySqlConnectOptions::new()
        .host("127.0.0.1")
        .port(3306)
        .pipes_as_concat(false)
        .no_engine_substitution(false)
        .timezone(None)
        .ssl_mode(MySqlSslMode::Disabled);
    if let Some(cred) = cred {
        let (user, pass) = cred.split_once(':').unwrap_or((cred, ""));
        opts = opts.username(user).password(pass);
    }
    if !netaddr.is_empty() {
        let (net, addr) = match netaddr.find('(') {
            Some(i) if netaddr.ends_with(')') => {
                (&netaddr[..i], &netaddr[i + 1..netaddr.len() - 1])
            }
            Some(_) => {
                return Err(err(
                    "invalid DSN: network address not terminated (missing closing brace)".into(),
                ))
            }
            None => (netaddr, ""),
        };
        if net != "tcp" {
            return Err(err(format!(
                "network {net} is not supported by ekiben, use tcp"
            )));
        }
        if !addr.is_empty() {
            let (host, port) =
                split_host_port(addr, 3306).ok_or_else(|| err("invalid address".into()))?;
            opts = opts.host(&host).port(port);
        }
    }
    if !dbname.is_empty() {
        opts = opts.database(&percent_decode(dbname));
    }
    let mut parse_time = false;
    for (k, v) in url::form_urlencoded::parse(params.as_bytes()) {
        let truthy = || matches!(v.as_ref(), "1" | "true" | "TRUE" | "True");
        opts = match k.as_ref() {
            "parseTime" => {
                parse_time = truthy();
                opts
            }
            "loc" if v == "UTC" => opts,
            "tls" => opts.ssl_mode(match v.as_ref() {
                "true" => MySqlSslMode::VerifyIdentity,
                "false" => MySqlSslMode::Disabled,
                "skip-verify" => MySqlSslMode::Required,
                "preferred" => MySqlSslMode::Preferred,
                other => {
                    return Err(err(format!(
                        "tls config {other} is not supported by ekiben"
                    )))
                }
            }),
            "charset" => opts.charset(v.split(',').next().unwrap_or_default()),
            "collation" => opts.collation(&v),
            "allowNativePasswords" if truthy() => opts,
            "timeout" | "readTimeout" | "writeTimeout" => {
                parse_duration_str(&v).map_err(|_| err(format!("{k}: invalid duration")))?;
                opts
            }
            other => return Err(err(format!("parameter {other} is not supported by ekiben"))),
        };
    }
    Ok((opts, parse_time))
}

fn split_host_port(addr: &str, default: u16) -> Option<(String, u16)> {
    if let Some(rest) = addr.strip_prefix('[') {
        let (h, p) = rest.split_once(']')?;
        let port = match p.strip_prefix(':') {
            Some(p) => p.parse().ok()?,
            None => default,
        };
        return Some((h.to_string(), port));
    }
    match addr.rsplit_once(':') {
        Some((h, p)) => Some((h.to_string(), p.parse().ok()?)),
        None => Some((addr.to_string(), default)),
    }
}

/// Parses `dsn` for `driver`. Returns the target and mysql's `parseTime`.
pub fn parse_dsn(
    driver: Driver,
    dsn: &str,
    root: Option<&Path>,
) -> Result<(Target, bool), CoreError> {
    Ok(match driver {
        Driver::Postgres => (Target::Postgres(parse_postgres_dsn(dsn)?), false),
        Driver::MySql => {
            let (o, pt) = parse_mysql_dsn(dsn)?;
            (Target::MySql(o), pt)
        }
        Driver::Sqlite => (Target::Sqlite(crate::sqlite::parse_dsn(dsn, root)?), false),
    })
}

/// `sqlOpenWithReworks`: judges the target host by the organization's outbound rule, opens a
/// pool with the settings' limits and, with `init_verify_conn`, pings it.
pub async fn open(
    target: &Target,
    settings: &ConnSettings,
    org: &Arc<OrgContext>,
) -> Result<Db, CoreError> {
    let (max_open, min_idle) = settings.pool_limits();
    macro_rules! pool {
        ($opts:ty) => {
            <$opts>::new()
                .max_connections(max_open)
                .min_connections(0)
                .idle_timeout(settings.conn_max_idle_time.filter(|d| !d.is_zero()))
                .max_lifetime(settings.conn_max_lifetime.filter(|d| !d.is_zero()))
                .test_before_acquire(false)
        };
    }
    let _ = min_idle;
    let db = match target {
        Target::Postgres(o) => {
            ekiben_io_http::outbound::resolve_host_checked(o.get_host(), o.get_port(), org).await?;
            Db::Postgres(pool!(PgPoolOptions).connect_lazy_with(o.clone()))
        }
        Target::MySql(o) => {
            ekiben_io_http::outbound::resolve_host_checked(o.get_host(), o.get_port(), org).await?;
            Db::MySql(pool!(MySqlPoolOptions).connect_lazy_with(o.clone()))
        }
        Target::Sqlite(t) => Db::Sqlite(crate::sqlite::SqliteDb::open(t).map_err(|e| {
            CoreError::other(format!("could not establish connection to database: {e}"))
        })?),
    };
    if settings.init_verify_conn {
        let ping = match &db {
            Db::Postgres(p) => p.acquire().await.map(drop),
            Db::MySql(p) => p.acquire().await.map(drop),
            Db::Sqlite(_) => Ok(()),
        };
        if let Err(e) = ping {
            db.close().await;
            return Err(CoreError::other(format!(
                "could not establish connection to database: {}",
                crate::util::db_error_text(&e)
            )));
        }
    }
    Ok(db)
}

/// `isAuthError`: SQLSTATE class 28 for postgres, "access denied" for mysql.
pub fn is_auth_error(driver: Driver, err: &crate::sqlx::Error) -> bool {
    match (driver, err) {
        (Driver::Postgres, crate::sqlx::Error::Database(db)) => {
            db.code().is_some_and(|c| c.starts_with("28"))
        }
        (Driver::MySql, e) => e.to_string().to_lowercase().contains("access denied"),
        _ => false,
    }
}

/// `driver.ErrBadConn`: the pool lost its connection.
pub fn is_bad_conn(err: &crate::sqlx::Error) -> bool {
    matches!(
        err,
        crate::sqlx::Error::Io(_)
            | crate::sqlx::Error::PoolClosed
            | crate::sqlx::Error::WorkerCrashed
            | crate::sqlx::Error::Tls(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_lint_names_the_driver() {
        assert!(Driver::parse("clickhouse")
            .unwrap_err()
            .contains("clickhouse"));
        assert_eq!(
            Driver::parse("pgx").unwrap_err(),
            "sql: unknown driver \"pgx\" (forgotten import?)"
        );
        assert_eq!(Driver::parse("sqlite"), Ok(Driver::Sqlite));
    }

    #[test]
    fn lint_refuses_listed_drivers_ekiben_cannot_run_only() {
        let lint = |d: &str| lint_conn(serde_json::json!({"driver": d}).as_object().unwrap());
        assert!(lint("pgx").is_empty(), "Bento does not lint the enum");
        assert!(lint("clickhouse")[0].contains("clickhouse"));
        assert!(lint("postgres").is_empty());
    }

    #[test]
    fn sql_conn_lint_rule() {
        let obj =
            serde_json::json!({"driver": "postgres", "iam_enabled": true, "secret_name": "x"});
        let errs = lint_conn(obj.as_object().unwrap());
        assert!(errs.iter().any(|e| e == SQL_CONN_LINT), "{errs:?}");
    }

    #[test]
    fn postgres_url_and_kv_dsn() {
        let o = parse_postgres_dsn(
            "postgres://foouser:foo%40pass@db.example.com:5433/foodb?sslmode=disable&search_path=x",
        )
        .unwrap();
        assert_eq!(o.get_host(), "db.example.com");
        assert_eq!(o.get_port(), 5433);
        assert_eq!(o.get_username(), "foouser");
        assert_eq!(o.get_database(), Some("foodb"));
        assert!(matches!(o.get_ssl_mode(), PgSslMode::Disable));
        let o = parse_postgres_dsn("host=h port=6000 user='a b' dbname=d").unwrap();
        assert_eq!(
            (o.get_host(), o.get_port(), o.get_username()),
            ("h", 6000, "a b")
        );
        assert!(
            matches!(o.get_ssl_mode(), PgSslMode::Require),
            "lib/pq requires TLS by default"
        );
        assert!(parse_postgres_dsn("host=/var/run/postgresql").is_err());
    }

    #[test]
    fn dsn_errors_never_echo_the_dsn() {
        let e = parse_postgres_dsn("postgres://u:s3cret@[bad")
            .unwrap_err()
            .to_string();
        assert!(!e.contains("s3cret"), "{e}");
        let e = parse_mysql_dsn("u:s3cret@tcp(h:1)/db?bogus=1")
            .unwrap_err()
            .to_string();
        assert!(!e.contains("s3cret") && e.contains("bogus"), "{e}");
    }

    #[test]
    fn mysql_dsn() {
        let (o, pt) =
            parse_mysql_dsn("foouser:foo@pass@tcp(localhost:3307)/foodb?parseTime=true").unwrap();
        assert!(pt);
        assert_eq!(
            (o.get_host(), o.get_port(), o.get_username()),
            ("localhost", 3307, "foouser")
        );
        assert_eq!(o.get_database(), Some("foodb"));
        let (o, pt) = parse_mysql_dsn("/db").unwrap();
        assert!(!pt);
        assert_eq!((o.get_host(), o.get_port()), ("127.0.0.1", 3306));
        assert!(parse_mysql_dsn("u@unix(/tmp/s)/db").is_err());
        assert!(parse_mysql_dsn("nodb").is_err());
    }

    #[test]
    fn pool_limits_follow_database_sql() {
        let obj = serde_json::json!({"conn_max_open": 3, "conn_max_idle": 10});
        let s = ConnSettings::from_config(obj.as_object().unwrap(), None).unwrap();
        assert_eq!(s.pool_limits(), (3, 3));
    }
}
