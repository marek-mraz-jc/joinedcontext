//! Port of `internal/impl/sql/output_sql_insert.go` (Bento 1.22.0): one multi-row
//! `INSERT INTO table (columns) VALUES (...),(...)` per batch (squirrel's `InsertBuilder`).

use async_trait::async_trait;
use ekiben_bloblang::Executor;
use ekiben_core::{
    lint_fields, Batch, ComponentConfig, CoreError, Output, Registry, StreamContext,
};
use serde_json::Value as JsonValue;
use tokio::sync::RwLock;

use crate::conn_fields::{self, CONN_FIELDS};
use crate::input_sql_raw::{refuse_lint, require, Conn};
use crate::input_sql_select::string_list;
use crate::output_sql_raw::{batch_args, lint_max_in_flight, OutConn};
use crate::util::{self, Placeholder};

const FIELDS: &[&str] = &[
    "driver",
    "dsn",
    "table",
    "columns",
    "args_mapping",
    "prefix",
    "suffix",
    "max_in_flight",
    "batching",
];

pub fn register(registry: &Registry) {
    registry.register_output("sql_insert", |conf, ctx| {
        Ok(std::sync::Arc::new(build(conf, ctx)?))
    });
    registry.register_lint("output", "sql_insert", lint);
}

pub fn lint(conf: &JsonValue) -> Vec<String> {
    let known: Vec<&str> = FIELDS.iter().chain(CONN_FIELDS).copied().collect();
    let mut errs = lint_fields(conf, &known, &[]);
    if let Some(obj) = conf.as_object() {
        errs.extend(conn_fields::lint_conn(obj));
        errs.extend(lint_max_in_flight(obj));
    }
    errs
}

/// squirrel `Insert(table).Columns(columns...).Values(row...)...Prefix(prefix).Suffix(suffix)`.
pub fn insert_sql(
    table: &str,
    columns: &[String],
    rows: &[usize],
    prefix: Option<&str>,
    suffix: Option<&str>,
    ph: Placeholder,
) -> Result<String, String> {
    if rows.is_empty() {
        return Err(
            "insert statements must have at least one set of values or select clause".into(),
        );
    }
    let mut sql = String::new();
    if let Some(p) = prefix {
        sql.push_str(p);
        sql.push(' ');
    }
    sql.push_str("INSERT INTO ");
    sql.push_str(table);
    sql.push(' ');
    if !columns.is_empty() {
        sql.push('(');
        sql.push_str(&columns.join(","));
        sql.push_str(") ");
    }
    sql.push_str("VALUES ");
    let values: Vec<String> = rows
        .iter()
        .map(|n| format!("({})", vec!["?"; *n].join(",")))
        .collect();
    sql.push_str(&values.join(","));
    if let Some(s) = suffix {
        sql.push(' ');
        sql.push_str(s);
    }
    Ok(ph.replace(&sql))
}

pub struct SqlInsertOutput {
    out: OutConn,
    table: String,
    columns: Vec<String>,
    prefix: Option<String>,
    suffix: Option<String>,
    args_mapping: Executor,
}

/// `newSQLInsertOutputFromConfig`.
pub fn build(conf: &ComponentConfig, ctx: &StreamContext) -> Result<SqlInsertOutput, CoreError> {
    refuse_lint("output", conf, lint)?;
    let obj = require(conf, &["driver", "dsn", "table", "columns", "args_mapping"])?;
    let conn = Conn::from_config(obj, "dsn", ctx)?;
    let s = |k: &str| obj.get(k).and_then(JsonValue::as_str).map(String::from);
    let mapping = obj
        .get("args_mapping")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| CoreError::config("field args_mapping: expected a mapping string"))?;
    let args_mapping = ctx.blobl_env.compile_mapping(mapping)?;
    Ok(SqlInsertOutput {
        out: OutConn {
            conn,
            db: RwLock::new(None),
        },
        table: s("table").unwrap_or_default(),
        columns: string_list(obj, "columns")?,
        prefix: s("prefix"),
        suffix: s("suffix"),
        args_mapping,
    })
}

#[async_trait]
impl Output for SqlInsertOutput {
    fn output_type(&self) -> &'static str {
        "sql_insert"
    }

    async fn connect(&self, ctx: &StreamContext) -> Result<(), CoreError> {
        self.out.connect(ctx).await
    }

    async fn write_batch(&self, _ctx: &StreamContext, mut batch: Batch) -> Result<(), CoreError> {
        let db = self.out.db().await?;
        for p in &mut batch {
            let _ = p.as_bytes();
        }
        let mut args = Vec::new();
        let mut rows = Vec::with_capacity(batch.len());
        for i in 0..batch.len() {
            let row = batch_args(&self.args_mapping, &batch, i)?;
            rows.push(row.len());
            args.extend(row);
        }
        let sql = insert_sql(
            &self.table,
            &self.columns,
            &rows,
            self.prefix.as_deref(),
            self.suffix.as_deref(),
            Placeholder::for_driver(self.out.conn.driver),
        )
        .map_err(CoreError::other)?;
        match util::exec(&db, &sql, &args).await {
            Ok(()) => Ok(()),
            Err(e) => Err(self.out.fail(db.driver(), e).await),
        }
    }

    fn is_connected(&self) -> bool {
        self.out.db.try_read().map(|g| g.is_some()).unwrap_or(true)
    }

    async fn close(&self) -> Result<(), CoreError> {
        self.out.close().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{conf, stream_ctx};
    use ekiben_core::Part;
    use serde_json::json;

    #[tokio::test]
    async fn sql_insert_writes_one_statement_per_batch() {
        let dir = tempfile::tempdir().unwrap();
        let dsn = dir.path().join("i.db").display().to_string();
        let ctx = stream_ctx(None);
        let out = build(
            &conf("sql_insert", json!({"driver": "sqlite", "dsn": dsn,
                "init_statement": "CREATE TABLE t (a int, b text)",
                "table": "t", "columns": ["a", "b"], "args_mapping": "root = [ this.a, this.b ]",
                "prefix": "WITH x AS (SELECT 1)", "suffix": "ON CONFLICT DO NOTHING"})),
            &ctx,
        )
        .unwrap();
        assert!(!out.is_connected());
        assert!(matches!(
            out.write_batch(&ctx, vec![Part::from_str("{}")]).await,
            Err(CoreError::NotConnected)
        ));
        out.connect(&ctx).await.unwrap();
        assert!(out.is_connected());
        out.write_batch(
            &ctx,
            vec![Part::from_str(r#"{"a":1,"b":"x"}"#), Part::from_str(r#"{"a":2,"b":"y"}"#)],
        )
        .await
        .unwrap();
        let n: i64 = rusqlite::Connection::open(&dsn)
            .unwrap()
            .query_row("SELECT sum(a) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 3);
        let e = out.write_batch(&ctx, Vec::new()).await.unwrap_err();
        assert!(e.to_string().contains("at least one set of values"), "{e}");
        let e = out
            .write_batch(&ctx, vec![Part::from_str(r#"{"a":[1]}"#)])
            .await
            .unwrap_err();
        assert!(e.to_string().contains("unsupported type []interface {}"), "{e}");
        let e = out
            .write_batch(&ctx, vec![Part::from_str("not json")])
            .await
            .unwrap_err();
        assert!(!e.to_string().is_empty());
        out.close().await.unwrap();
        assert!(!out.is_connected());
    }

    #[test]
    fn sql_insert_needs_a_mapping_string_and_a_list_of_columns() {
        let ctx = stream_ctx(None);
        let base = json!({"driver": "sqlite", "dsn": ":memory:", "table": "t",
            "columns": ["a"], "args_mapping": "root = [1]"});
        let with = |k: &str, v: serde_json::Value| {
            let mut c = base.clone();
            c[k] = v;
            build(&conf("sql_insert", c), &ctx).err().expect(k).to_string()
        };
        assert!(with("args_mapping", json!(5)).contains("field args_mapping: expected a mapping string"));
        assert!(with("columns", json!("a")).contains("field columns: expected a list of strings"));
        assert!(with("columns", json!(["a", 1])).contains("field columns: expected a list of strings"));
        assert!(with("max_in_flight", json!(1.5)).contains("max_in_flight: expected an integer"));
        assert_eq!(
            insert_sql("t", &[], &[1], Some("P"), None, Placeholder::Question).unwrap(),
            "P INSERT INTO t VALUES (?)"
        );
    }

    #[test]
    fn squirrel_insert() {
        let cols = vec!["foo".to_string(), "bar".to_string()];
        assert_eq!(
            insert_sql(
                "t",
                &cols,
                &[2, 2],
                None,
                Some("ON CONFLICT (foo) DO NOTHING"),
                Placeholder::Dollar
            )
            .unwrap(),
            "INSERT INTO t (foo,bar) VALUES ($1,$2),($3,$4) ON CONFLICT (foo) DO NOTHING"
        );
        assert_eq!(
            insert_sql("t", &cols, &[0], None, None, Placeholder::Question).unwrap(),
            "INSERT INTO t (foo,bar) VALUES ()"
        );
        assert!(insert_sql("t", &cols, &[], None, None, Placeholder::Question).is_err());
    }
}
