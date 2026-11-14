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
    args_mapping: Option<Executor>,
}

/// `newSQLInsertOutputFromConfig`.
pub fn build(conf: &ComponentConfig, ctx: &StreamContext) -> Result<SqlInsertOutput, CoreError> {
    refuse_lint("output", conf, lint)?;
    require(conf, &["driver", "dsn", "table", "columns", "args_mapping"])?;
    let obj = conf
        .config
        .as_object()
        .ok_or_else(|| CoreError::config("sql_insert: expected an object"))?;
    let conn = Conn::from_config(obj, "dsn", ctx)?;
    let s = |k: &str| obj.get(k).and_then(JsonValue::as_str).map(String::from);
    let args_mapping = match obj.get("args_mapping").and_then(JsonValue::as_str) {
        Some(m) => Some(ctx.blobl_env.compile_mapping(m)?),
        None => None,
    };
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
            let row = match &self.args_mapping {
                Some(exec) => batch_args(exec, &batch, i)?,
                None => Vec::new(),
            };
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
