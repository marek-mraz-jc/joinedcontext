//! The `sqlite` driver: modernc.org/sqlite's behaviour over rusqlite (bundled SQLite 3.54, which
//! carries the fix for RUSTSEC-2026-0459; sqlx 0.9's sqlite pins the affected libsqlite3-sys).
//! One connection per component behind a mutex, used on the blocking pool: SQLite serializes
//! writes, and a `:memory:` database lives as long as its one connection.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, TimeZone, Utc};
use ekiben_bloblang::Value;
use ekiben_core::CoreError;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::limits::Limit;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};

use crate::conn_fields::{dsn_error, Driver};

/// What a modernc DSN opens.
#[derive(Clone, Debug)]
pub struct SqliteTarget {
    /// `None` is an in-memory database.
    pub path: Option<PathBuf>,
    pub flags: OpenFlags,
    pub pragmas: Vec<(String, String)>,
}

/// Parses a modernc sqlite DSN (`file:/path/foo.db?_pragma=foreign_keys(1)`, a plain path, or
/// `:memory:`); the file stays under `root`.
pub fn parse_dsn(dsn: &str, root: Option<&Path>) -> Result<SqliteTarget, CoreError> {
    let err = |why: String| dsn_error(Driver::Sqlite, why);
    let body = dsn.strip_prefix("file:").unwrap_or(dsn);
    let (path, params) = body.split_once('?').unwrap_or((body, ""));
    let mut t = SqliteTarget {
        path: if path == ":memory:" || path.is_empty() {
            None
        } else {
            Some(ekiben_io_file::path_guard::sanitize_file_path(path, root)?)
        },
        flags: OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        pragmas: Vec::new(),
    };
    let ident = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    for (k, v) in url::form_urlencoded::parse(params.as_bytes()) {
        match k.as_ref() {
            "_pragma" => {
                let (name, val) = v
                    .split_once('(')
                    .and_then(|(n, rest)| {
                        rest.strip_suffix(')')
                            .map(|r| (n.to_string(), r.to_string()))
                    })
                    .unwrap_or_else(|| (v.to_string(), String::new()));
                let val_ok = val
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_-.'".contains(c));
                if !ident(&name) || !val_ok {
                    return Err(err(format!("invalid _pragma {v}")));
                }
                t.pragmas.push((name, val));
            }
            "mode" => match v.as_ref() {
                "ro" => {
                    t.flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX
                }
                "rw" => t.flags.remove(OpenFlags::SQLITE_OPEN_CREATE),
                "rwc" => {}
                "memory" => t.path = None,
                other => return Err(err(format!("mode {other} is not supported"))),
            },
            "cache" if v == "shared" => t.flags |= OpenFlags::SQLITE_OPEN_SHARED_CACHE,
            "cache" if v == "private" => t.flags |= OpenFlags::SQLITE_OPEN_PRIVATE_CACHE,
            other => return Err(err(format!("parameter {other} is not supported by ekiben"))),
        }
    }
    Ok(t)
}

/// One open sqlite database.
#[derive(Clone)]
pub struct SqliteDb(Arc<Mutex<Option<Connection>>>);

fn msg(e: rusqlite::Error) -> String {
    e.to_string()
}

impl SqliteDb {
    pub fn open(t: &SqliteTarget) -> Result<Self, String> {
        let mut conn = match &t.path {
            Some(p) => Connection::open_with_flags(p, t.flags),
            None => Connection::open_in_memory_with_flags(t.flags),
        }
        .map_err(msg)?;
        // The DSN's path was held under the file root; ATTACH DATABASE and VACUUM INTO (which
        // attaches its target) in a query or init_statement would name any other path. No
        // database may be attached, and the authorizer refuses the statement while it compiles.
        conn.set_limit(Limit::SQLITE_LIMIT_ATTACHED, 0)
            .map_err(msg)?;
        conn.authorizer(Some(|ctx: AuthContext<'_>| match ctx.action {
            AuthAction::Attach { .. } | AuthAction::Detach { .. } => Authorization::Deny,
            _ => Authorization::Allow,
        }))
        .map_err(msg)?;
        for (name, val) in &t.pragmas {
            // Both parts were checked to be identifier and literal characters in parse_dsn.
            let sql = if val.is_empty() {
                format!("PRAGMA {name}")
            } else {
                format!("PRAGMA {name} = {val}")
            };
            conn.execute_batch(&sql).map_err(msg)?;
        }
        Ok(Self(Arc::new(Mutex::new(Some(conn)))))
    }

    /// Runs `f` on the connection on the blocking pool.
    pub async fn with<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let db = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let guard = db.lock().unwrap_or_else(PoisonError::into_inner);
            match guard.as_ref() {
                Some(c) => f(c),
                None => Err("sql: database is closed".to_string()),
            }
        })
        .await
        .map_err(|e| e.to_string())?
    }

    pub fn close(&self) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take();
    }
}

/// modernc's argument binding: int64, float64, bool as 0/1, string, []byte, time.Time as text.
fn to_sql(v: &Value) -> rusqlite::types::Value {
    use rusqlite::types::Value as S;
    match v {
        Value::Null | Value::Nothing | Value::Delete | Value::Array(_) | Value::Object(_) => {
            S::Null
        }
        Value::Bool(b) => S::Integer(i64::from(*b)),
        Value::Int(n) => S::Integer(*n),
        Value::Uint(n) => S::Integer(*n as i64),
        Value::Float(f) => S::Real(*f),
        Value::Number(s) | Value::String(s) => S::Text(s.clone()),
        Value::Bytes(b) => S::Blob(b.clone()),
        Value::Timestamp(t) => S::Text(time_text(t)),
    }
}

/// modernc writes a time.Time as "2006-01-02 15:04:05.999999999-07:00".
pub fn time_text(t: &DateTime<FixedOffset>) -> String {
    let mut s = t.format("%Y-%m-%d %H:%M:%S").to_string();
    let nanos = t.timestamp_subsec_nanos();
    if nanos > 0 {
        s.push('.');
        s.push_str(format!("{nanos:09}").trim_end_matches('0'));
    }
    s.push_str(&t.format("%:z").to_string());
    s
}

/// modernc's `parseTime` formats, tried in order.
pub fn parse_time(s: &str) -> Option<DateTime<FixedOffset>> {
    let s = s.trim_end_matches('Z');
    for f in ["%Y-%m-%d %H:%M:%S%.f%:z", "%Y-%m-%dT%H:%M:%S%.f%:z"] {
        if let Ok(t) = DateTime::parse_from_str(s, f) {
            return Some(t);
        }
    }
    for f in [
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M",
    ] {
        if let Ok(t) = NaiveDateTime::parse_from_str(s, f) {
            return Some(Utc.from_utc_datetime(&t).fixed_offset());
        }
    }
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|t| Utc.from_utc_datetime(&t).fixed_offset())
}

/// A column value as modernc decodes it: INTEGER int64, REAL float64, TEXT string, BLOB bytes
/// (read on as a string); TEXT in a column declared DATE, DATETIME, TIME or TIMESTAMP is parsed.
fn column_value(v: ValueRef<'_>, declared: Option<&str>) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(n) => Value::Int(n),
        ValueRef::Real(f) => Value::Float(f),
        ValueRef::Blob(b) => Value::String(String::from_utf8_lossy(b).into_owned()),
        ValueRef::Text(t) => {
            let s = String::from_utf8_lossy(t).into_owned();
            let timed = declared
                .map(str::to_ascii_uppercase)
                .is_some_and(|d| matches!(d.as_str(), "DATE" | "DATETIME" | "TIMESTAMP" | "TIME"));
            match timed.then(|| parse_time(&s)).flatten() {
                Some(t) => Value::Timestamp(t),
                None => Value::String(s),
            }
        }
    }
}

/// Runs `sql` with `args` for its rows, handing each row object to `each` until it returns false.
pub fn query(
    conn: &Connection,
    sql: &str,
    args: &[Value],
    mut each: impl FnMut(Value) -> bool,
) -> Result<(), String> {
    let mut stmt = conn.prepare(sql).map_err(msg)?;
    let cols: Vec<(String, Option<String>)> = stmt
        .columns()
        .map(|c| (c.name().to_string(), c.decl_type().map(String::from)))
        .collect();
    let params: Vec<rusqlite::types::Value> = args.iter().map(to_sql).collect();
    let mut rows = stmt
        .query(rusqlite::params_from_iter(params))
        .map_err(msg)?;
    while let Some(row) = rows.next().map_err(msg)? {
        let mut obj = std::collections::BTreeMap::new();
        for (i, (name, decl)) in cols.iter().enumerate() {
            obj.insert(
                name.clone(),
                column_value(row.get_ref(i).map_err(msg)?, decl.as_deref()),
            );
        }
        if !each(Value::Object(obj.into())) {
            break;
        }
    }
    Ok(())
}

/// `ExecContext`: without arguments every statement of `sql` runs, as modernc does.
pub fn exec(conn: &Connection, sql: &str, args: &[Value]) -> Result<(), String> {
    if args.is_empty() {
        return conn.execute_batch(sql).map_err(msg);
    }
    let params: Vec<rusqlite::types::Value> = args.iter().map(to_sql).collect();
    conn.execute(sql, rusqlite::params_from_iter(params))
        .map(drop)
        .map_err(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_dsn_stays_under_root() {
        let dir = tempfile::tempdir().unwrap();
        let t = parse_dsn("file:foo.db?_pragma=foreign_keys(1)", Some(dir.path())).unwrap();
        assert!(t.path.unwrap().starts_with(dir.path()));
        assert_eq!(
            t.pragmas,
            vec![("foreign_keys".to_string(), "1".to_string())]
        );
        assert!(parse_dsn("file:../x.db", Some(dir.path())).is_err());
        assert!(parse_dsn("x.db?_pragma=a;drop", None).is_err());
        assert!(parse_dsn("x.db?_pragma=journal_mode(wal;drop)", None).is_err());
        assert!(parse_dsn(":memory:", None).unwrap().path.is_none());
    }

    #[test]
    fn attach_and_vacuum_into_cannot_leave_the_root() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let db = SqliteDb::open(&parse_dsn("file:in.db", Some(root.path())).unwrap()).unwrap();
        let guard = db.0.lock().unwrap();
        let c = guard.as_ref().unwrap();
        exec(
            c,
            "CREATE TABLE t (a integer); INSERT INTO t VALUES (1)",
            &[],
        )
        .unwrap();
        for sql in [
            format!(
                "ATTACH DATABASE '{}' AS x",
                outside.path().join("a.db").display()
            ),
            format!("VACUUM INTO '{}'", outside.path().join("v.db").display()),
            format!(
                "VACUUM main INTO '{}'",
                outside.path().join("w.db").display()
            ),
            "ATTACH ':memory:' AS m".to_string(),
            "DETACH DATABASE main".to_string(),
        ] {
            assert!(exec(c, &sql, &[]).is_err(), "{sql} ran");
        }
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
        let mut n = 0;
        query(c, "SELECT a FROM t", &[], |_| {
            n += 1;
            true
        })
        .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn rows_decode_as_modernc() {
        let db = SqliteDb::open(&parse_dsn(":memory:", None).unwrap()).unwrap();
        let guard = db.0.lock().unwrap();
        let c = guard.as_ref().unwrap();
        exec(c, "CREATE TABLE t (a integer, b real, c text, d blob, e datetime); INSERT INTO t VALUES (1, 1.5, 'x', x'41', '2024-01-02 03:04:05')", &[]).unwrap();
        let mut got = Vec::new();
        query(c, "SELECT * FROM t WHERE a = ?", &[Value::Int(1)], |v| {
            got.push(v.to_go_json());
            true
        })
        .unwrap();
        assert_eq!(
            got,
            vec![r#"{"a":1,"b":1.5,"c":"x","d":"A","e":"2024-01-02T03:04:05Z"}"#]
        );
    }

    #[test]
    fn sqlite_times() {
        let t = DateTime::parse_from_rfc3339("2024-01-02T03:04:05Z").unwrap();
        assert_eq!(time_text(&t), "2024-01-02 03:04:05+00:00");
        assert!(parse_time("2024-01-02 03:04:05").is_some());
        assert!(parse_time("not a time").is_none());
    }
}
