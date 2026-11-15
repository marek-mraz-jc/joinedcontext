//! `sql` cache (Bento 1.22.0 `internal/impl/sql/cache_sql.go`): each key is a row of `table`,
//! the key in `key_column` and the value in `value_column`. `get` selects, `delete` deletes, `add`
//! inserts (a duplicate key is `KeyAlreadyExists`), `set` inserts with `set_suffix` appended, the
//! engine's upsert (`ON CONFLICT (k) DO UPDATE SET v=excluded.v`). As in Bento there is no TTL and
//! the table is the author's: nothing is created. Because rows live in the database, `dedupe` and
//! `cached` keep what they saw across runner restarts (PL-51).
//!
//! ekiben deviations: `table` and the two columns are plain identifiers (they are written into the
//! statements), the DSN goes through the organization's outbound rule, as every sql DSN does, and
//! a value must be UTF-8: the row reader reads a value back as text (as `sql_select` does), so a
//! binary value is refused when written rather than returned altered.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use ekiben_bloblang::Value;
use ekiben_core::{
    lint_fields, Cache, ComponentConfig, CoreError, OrgContext, Registry, StreamContext,
};
use serde_json::Value as JsonValue;

use crate::conn_fields::{self, CONN_FIELDS};
use crate::input_sql_raw::{refuse_lint, require, Conn};
use crate::processor_sql::{query_rows, LazyDb};
use crate::util::{self, Placeholder};

const FIELDS: &[&str] = &[
    "driver",
    "dsn",
    "table",
    "key_column",
    "value_column",
    "set_suffix",
];

pub fn register(registry: &Registry) {
    registry.register_cache("sql", |conf, ctx| Ok(Arc::new(build(conf, ctx)?)));
    registry.register_lint("cache", "sql", lint);
}

/// A plain, unquoted identifier: it is written into the statements.
fn valid_ident(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 63
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub fn lint(conf: &JsonValue) -> Vec<String> {
    let known: Vec<&str> = FIELDS.iter().chain(CONN_FIELDS).copied().collect();
    let mut errs = lint_fields(conf, &known, &[]);
    if let Some(obj) = conf.as_object() {
        errs.extend(conn_fields::lint_conn(obj));
        for field in ["table", "key_column", "value_column"] {
            if let Some(t) = obj.get(field).and_then(JsonValue::as_str) {
                if !valid_ident(t) {
                    errs.push(format!(
                        "field {field}: '{t}' is not a plain name (letters, digits, _; at most 63)"
                    ));
                }
            }
        }
    }
    errs
}

pub fn build(conf: &ComponentConfig, ctx: &StreamContext) -> Result<SqlCache, CoreError> {
    refuse_lint("cache", conf, lint)?;
    require(
        conf,
        &["driver", "dsn", "table", "key_column", "value_column"],
    )?;
    let obj = conf
        .config
        .as_object()
        .ok_or_else(|| CoreError::config("sql cache: expected an object"))?;
    let text = |k: &str| {
        obj.get(k)
            .and_then(JsonValue::as_str)
            .unwrap_or_default()
            .to_string()
    };
    Ok(SqlCache::new(
        Conn::from_parts(obj, "dsn", ctx.file_root.as_deref())?,
        Columns {
            table: text("table"),
            key: text("key_column"),
            value: text("value_column"),
            set_suffix: text("set_suffix"),
        },
        ctx.org.clone(),
    ))
}

/// Where the rows live: `table (key, value)` and the upsert suffix of `set`.
pub struct Columns {
    pub table: String,
    pub key: String,
    pub value: String,
    pub set_suffix: String,
}

pub struct SqlCache {
    db: LazyDb,
    org: Arc<OrgContext>,
    cols: Columns,
    ph: Placeholder,
}

/// A unique-key refusal, in the words each engine uses (sqlite's driver keeps only the text).
fn is_duplicate(e: &crate::sqlx::Error) -> bool {
    if let crate::sqlx::Error::Database(d) = e {
        if d.is_unique_violation() {
            return true;
        }
    }
    let text = util::db_error_text(e);
    [
        "duplicate key",
        "UNIQUE constraint failed",
        "Duplicate entry",
    ]
    .iter()
    .any(|m| text.contains(m))
}

impl SqlCache {
    pub fn new(conn: Conn, cols: Columns, org: Arc<OrgContext>) -> Self {
        Self {
            ph: Placeholder::for_driver(conn.driver),
            db: LazyDb::new(conn),
            org,
            cols,
        }
    }

    fn insert(&self) -> String {
        let Columns {
            table, key, value, ..
        } = &self.cols;
        self.ph
            .replace(&format!("INSERT INTO {table} ({key},{value}) VALUES (?,?)"))
    }

    fn sql_err(e: impl std::fmt::Display) -> CoreError {
        CoreError::other(format!("sql cache: {e}"))
    }

    fn args(key: &str, value: &Bytes) -> Result<[Value; 2], CoreError> {
        let text = std::str::from_utf8(value).map_err(|_| {
            Self::sql_err(format!(
                "the value of '{key}' is not UTF-8 text, which is all this cache stores"
            ))
        })?;
        Ok([
            Value::String(key.to_string()),
            Value::String(text.to_string()),
        ])
    }
}

#[async_trait]
impl Cache for SqlCache {
    async fn get(&self, key: &str) -> Result<Bytes, CoreError> {
        let db = self.db.get(&self.org).await?;
        let Columns {
            table,
            key: kc,
            value: vc,
            ..
        } = &self.cols;
        let sql = self
            .ph
            .replace(&format!("SELECT {vc} FROM {table} WHERE {kc} = ?"));
        let rows = query_rows(&db, &sql, vec![Value::String(key.to_string())], false, 1)
            .await
            .map_err(Self::sql_err)?;
        let Some(row) = rows.into_iter().next() else {
            return Err(CoreError::KeyNotFound(key.to_string()));
        };
        match row.get_path(std::slice::from_ref(vc)) {
            Value::Bytes(b) => Ok(Bytes::from(b)),
            Value::String(s) => Ok(Bytes::from(s.into_bytes())),
            Value::Null => Ok(Bytes::new()),
            other => Ok(Bytes::from(other.to_string().into_bytes())),
        }
    }

    async fn set(&self, key: &str, value: Bytes, _ttl: Option<Duration>) -> Result<(), CoreError> {
        let db = self.db.get(&self.org).await?;
        let sql = match self.cols.set_suffix.as_str() {
            "" => self.insert(),
            suffix => format!("{} {suffix}", self.insert()),
        };
        let args = Self::args(key, &value)?;
        util::exec(&db, &sql, &args)
            .await
            .map_err(|e| Self::sql_err(util::db_error_text(&e)))
    }

    async fn add(&self, key: &str, value: Bytes, _ttl: Option<Duration>) -> Result<(), CoreError> {
        let db = self.db.get(&self.org).await?;
        let args = Self::args(key, &value)?;
        match util::exec(&db, &self.insert(), &args).await {
            Ok(()) => Ok(()),
            Err(e) if is_duplicate(&e) => Err(CoreError::KeyAlreadyExists(key.to_string())),
            Err(e) => Err(Self::sql_err(util::db_error_text(&e))),
        }
    }

    async fn delete(&self, key: &str) -> Result<(), CoreError> {
        let db = self.db.get(&self.org).await?;
        let Columns { table, key: kc, .. } = &self.cols;
        let sql = self
            .ph
            .replace(&format!("DELETE FROM {table} WHERE {kc} = ?"));
        util::exec(&db, &sql, &[Value::String(key.to_string())])
            .await
            .map_err(|e| Self::sql_err(util::db_error_text(&e)))
    }

    async fn close(&self) -> Result<(), CoreError> {
        self.db.close().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ekiben_core::{OrgId, OrgQuotas};
    use serde_json::json;

    fn cache(dsn: &str, set_suffix: &str) -> SqlCache {
        let conf = json!({"driver": "sqlite", "dsn": dsn});
        let conn = Conn::from_parts(conf.as_object().unwrap(), "dsn", None).unwrap();
        SqlCache::new(
            conn,
            Columns {
                table: "kv".into(),
                key: "k".into(),
                value: "v".into(),
                set_suffix: set_suffix.into(),
            },
            OrgContext::new(OrgId::new("o"), OrgQuotas::default()),
        )
    }

    async fn table(dsn: &str) {
        let c = cache(dsn, "");
        let db = c.db.get(&c.org).await.unwrap();
        db.exec_raw("CREATE TABLE kv (k TEXT PRIMARY KEY, v BLOB)")
            .await
            .unwrap();
        c.close().await.unwrap();
    }

    #[tokio::test]
    async fn a_key_added_before_a_restart_is_still_there_after() {
        let dir = tempfile::tempdir().unwrap();
        let dsn = dir.path().join("c.db").display().to_string();
        table(&dsn).await;
        assert!(cache(&dsn, "")
            .add("bin", Bytes::from_static(b"\xff"), None)
            .await
            .is_err());
        let value = Bytes::from_static("\u{0}ü v".as_bytes());
        let first = cache(&dsn, "");
        first.add("k", value.clone(), None).await.unwrap();
        assert!(matches!(
            first.add("k", value.clone(), None).await,
            Err(CoreError::KeyAlreadyExists(_))
        ));
        first.close().await.unwrap();

        // A new runner: dedupe's `add` still refuses the key it saw.
        let second = cache(&dsn, "");
        assert_eq!(second.get("k").await.unwrap(), value);
        assert!(matches!(
            second.add("k", value.clone(), None).await,
            Err(CoreError::KeyAlreadyExists(_))
        ));
        second.delete("k").await.unwrap();
        assert!(matches!(
            second.get("k").await,
            Err(CoreError::KeyNotFound(_))
        ));
    }

    #[tokio::test]
    async fn set_appends_the_suffix_and_without_one_acts_as_add() {
        let dir = tempfile::tempdir().unwrap();
        let dsn = dir.path().join("c.db").display().to_string();
        table(&dsn).await;
        let plain = cache(&dsn, "");
        plain
            .set("k", Bytes::from_static(b"1"), None)
            .await
            .unwrap();
        assert!(plain
            .set("k", Bytes::from_static(b"2"), None)
            .await
            .is_err());
        let upsert = cache(&dsn, "ON CONFLICT (k) DO UPDATE SET v=excluded.v");
        upsert
            .set("k", Bytes::from_static(b"3"), None)
            .await
            .unwrap();
        assert_eq!(upsert.get("k").await.unwrap(), Bytes::from_static(b"3"));
    }

    #[test]
    fn lint_names_what_the_cache_refuses() {
        assert!(valid_ident("pipeline_changes"));
        for bad in ["", "1x", "a;drop", "a.b", &"x".repeat(64)] {
            assert!(!valid_ident(bad), "{bad}");
        }
        let errs = lint(&json!({"driver": "sqlite", "dsn": "x", "table": "a b",
            "key_column": "k", "value_column": "v;", "default_ttl": "1h"}));
        assert!(errs.iter().any(|e| e.contains("table")), "{errs:?}");
        assert!(errs.iter().any(|e| e.contains("value_column")), "{errs:?}");
        assert!(errs.iter().any(|e| e.contains("default_ttl")), "{errs:?}");
    }
}
