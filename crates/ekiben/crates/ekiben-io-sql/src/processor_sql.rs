//! Port of the sql processors of Bento 1.22.0 (`internal/impl/sql/processor_sql_raw.go`,
//! `processor_sql_select.go`, `processor_sql_insert.go`) over the connection, statements and row
//! reading the sql inputs and outputs use.
//!
//! - `sql_raw` and `sql_select` replace each message with the array of rows its query answers;
//!   `sql_raw` with `exec_only` runs the statement and keeps the message.
//! - `sql_insert` inserts the whole batch in one statement and keeps the messages.
//! - A failed statement flags its message (every message for `sql_insert`), as in Bento.
//!
//! ekiben deviations: `unsafe_dynamic_query` is refused (a message never writes SQL text), and a
//! query answering more rows than the organization's `max_in_flight_messages` flags its message
//! rather than holding them all. The pool opens on the first batch, once the DSN's host passed
//! the outbound rule.

use std::sync::Arc;

use async_trait::async_trait;
use ekiben_bloblang::{Executor, Value};
use ekiben_core::{
    lint_fields, Batch, ComponentConfig, CoreError, OrgContext, ProcContext, Processor, Registry,
};
use serde_json::Value as JsonValue;
use tokio::sync::{mpsc, Mutex};

use crate::conn_fields::{self, Db, CONN_FIELDS};
use crate::input_sql_raw::{refuse_lint, require, Conn};
use crate::input_sql_select::{select_sql, string_list};
use crate::output_sql_insert::insert_sql;
use crate::output_sql_raw::batch_args;
use crate::util::{self, Placeholder};

const RAW_FIELDS: &[&str] = &[
    "driver",
    "dsn",
    "query",
    "unsafe_dynamic_query",
    "args_mapping",
    "exec_only",
];
const SELECT_FIELDS: &[&str] = &[
    "driver",
    "dsn",
    "table",
    "columns",
    "where",
    "args_mapping",
    "prefix",
    "suffix",
];
const INSERT_FIELDS: &[&str] = &[
    "driver",
    "dsn",
    "table",
    "columns",
    "args_mapping",
    "prefix",
    "suffix",
];

pub fn register(registry: &Registry) {
    registry.register_processor("sql_raw", |conf, ctx| Ok(Arc::new(build_raw(conf, ctx)?)));
    registry.register_processor("sql_select", |conf, ctx| {
        Ok(Arc::new(build_select(conf, ctx)?))
    });
    registry.register_processor("sql_insert", |conf, ctx| {
        Ok(Arc::new(build_insert(conf, ctx)?))
    });
    registry.register_lint("processor", "sql_raw", lint_raw);
    registry.register_lint("processor", "sql_select", lint_select);
    registry.register_lint("processor", "sql_insert", lint_insert);
}

fn lint_with(conf: &JsonValue, fields: &[&str]) -> Vec<String> {
    let known: Vec<&str> = fields.iter().chain(CONN_FIELDS).copied().collect();
    let mut errs = lint_fields(conf, &known, &[]);
    if let Some(obj) = conf.as_object() {
        errs.extend(conn_fields::lint_conn(obj));
    }
    errs
}

pub fn lint_raw(conf: &JsonValue) -> Vec<String> {
    let mut errs = lint_with(conf, RAW_FIELDS);
    if conf
        .get("unsafe_dynamic_query")
        .and_then(JsonValue::as_bool)
        == Some(true)
    {
        errs.push(
            "field unsafe_dynamic_query is not supported by ekiben: a message never writes SQL text"
                .to_string(),
        );
    }
    errs
}

pub fn lint_select(conf: &JsonValue) -> Vec<String> {
    lint_with(conf, SELECT_FIELDS)
}

pub fn lint_insert(conf: &JsonValue) -> Vec<String> {
    lint_with(conf, INSERT_FIELDS)
}

/// A pool opened on first use and kept for the component's life.
pub(crate) struct LazyDb {
    pub conn: Conn,
    db: Mutex<Option<Db>>,
}

impl LazyDb {
    pub fn new(conn: Conn) -> Self {
        Self {
            conn,
            db: Mutex::new(None),
        }
    }

    pub async fn get(&self, org: &Arc<OrgContext>) -> Result<Db, CoreError> {
        let mut guard = self.db.lock().await;
        if let Some(db) = guard.as_ref() {
            return Ok(db.clone());
        }
        let db = self.conn.open_for(org).await?;
        *guard = Some(db.clone());
        Ok(db)
    }

    pub async fn close(&self) {
        if let Some(db) = self.db.lock().await.take() {
            db.close().await;
        }
    }
}

/// The rows `sql` answers, at most `max_rows` of them (one more is an error, never a cut).
pub(crate) async fn query_rows(
    db: &Db,
    sql: &str,
    args: Vec<Value>,
    parse_time: bool,
    max_rows: usize,
) -> Result<Vec<Value>, String> {
    let (tx, mut rx) = mpsc::channel(16);
    tokio::spawn(util::stream_rows(
        db.clone(),
        sql.to_string(),
        args,
        parse_time,
        tx,
    ));
    let mut rows = Vec::new();
    while let Some(row) = rx.recv().await {
        let row = row?;
        if rows.len() >= max_rows {
            // Dropping the receiver stops the reader.
            return Err(format!(
                "query answered more than {max_rows} rows, the most one message may hold"
            ));
        }
        rows.push(row);
    }
    Ok(rows)
}

fn object(conf: &ComponentConfig) -> Result<&serde_json::Map<String, JsonValue>, CoreError> {
    conf.config
        .as_object()
        .ok_or_else(|| CoreError::config(format!("{}: expected an object", conf.component_type)))
}

fn args_mapping(
    obj: &serde_json::Map<String, JsonValue>,
    ctx: &ProcContext,
) -> Result<Option<Executor>, CoreError> {
    match obj.get("args_mapping").and_then(JsonValue::as_str) {
        Some(m) => Ok(Some(ctx.blobl_env.compile_mapping(m)?)),
        None => Ok(None),
    }
}

/// `sql_raw` and `sql_select`.
pub struct SqlQueryProcessor {
    name: &'static str,
    db: LazyDb,
    query: String,
    args_mapping: Option<Executor>,
    exec_only: bool,
}

/// `newSQLRawProcessorFromConfig`.
pub fn build_raw(
    conf: &ComponentConfig,
    ctx: &ProcContext,
) -> Result<SqlQueryProcessor, CoreError> {
    refuse_lint("processor", conf, lint_raw)?;
    require(conf, &["driver", "dsn", "query"])?;
    let obj = object(conf)?;
    Ok(SqlQueryProcessor {
        name: "sql_raw",
        db: LazyDb::new(Conn::from_parts(obj, "dsn", ctx.file_root.as_deref())?),
        query: obj
            .get("query")
            .and_then(JsonValue::as_str)
            .unwrap_or_default()
            .to_string(),
        args_mapping: args_mapping(obj, ctx)?,
        exec_only: obj
            .get("exec_only")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false),
    })
}

/// `newSQLSelectProcessorFromConfig`.
pub fn build_select(
    conf: &ComponentConfig,
    ctx: &ProcContext,
) -> Result<SqlQueryProcessor, CoreError> {
    refuse_lint("processor", conf, lint_select)?;
    require(conf, &["driver", "dsn", "table", "columns"])?;
    let obj = object(conf)?;
    let conn = Conn::from_parts(obj, "dsn", ctx.file_root.as_deref())?;
    let s = |k: &str| obj.get(k).and_then(JsonValue::as_str);
    let query = select_sql(
        &string_list(obj, "columns")?,
        s("table").unwrap_or_default(),
        s("where").unwrap_or_default(),
        s("prefix"),
        s("suffix"),
        Placeholder::for_driver(conn.driver),
    );
    Ok(SqlQueryProcessor {
        name: "sql_select",
        db: LazyDb::new(conn),
        query,
        args_mapping: args_mapping(obj, ctx)?,
        exec_only: false,
    })
}

impl SqlQueryProcessor {
    async fn run(
        &self,
        db: &Db,
        batch: &Batch,
        i: usize,
        max_rows: usize,
    ) -> Result<Option<Vec<Value>>, String> {
        let args = match &self.args_mapping {
            Some(exec) => batch_args(exec, batch, i).map_err(|e| e.to_string())?,
            None => Vec::new(),
        };
        if self.exec_only {
            return util::exec(db, &self.query, &args)
                .await
                .map(|()| None)
                .map_err(|e| util::db_error_text(&e));
        }
        query_rows(db, &self.query, args, self.db.conn.parse_time, max_rows)
            .await
            .map(Some)
    }
}

#[async_trait]
impl Processor for SqlQueryProcessor {
    fn processor_type(&self) -> &'static str {
        self.name
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        mut batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        if batch.is_empty() {
            return Ok(Vec::new());
        }
        let db = self.db.get(&ctx.org).await?;
        let max_rows = ctx.org.get_quotas().max_in_flight_messages;
        for p in &mut batch {
            let _ = p.as_bytes();
        }
        for i in 0..batch.len() {
            match self.run(&db, &batch, i, max_rows).await {
                Ok(Some(rows)) => {
                    batch[i].set_structured(Value::Array(rows.into()));
                }
                Ok(None) => {}
                Err(e) => ctx.on_error(&mut batch[i], e),
            }
        }
        Ok(vec![batch])
    }

    async fn close(&self) -> Result<(), CoreError> {
        self.db.close().await;
        Ok(())
    }
}

/// `sql_insert` processor.
pub struct SqlInsertProcessor {
    db: LazyDb,
    table: String,
    columns: Vec<String>,
    prefix: Option<String>,
    suffix: Option<String>,
    args_mapping: Executor,
}

/// `newSQLInsertProcessorFromConfig`.
pub fn build_insert(
    conf: &ComponentConfig,
    ctx: &ProcContext,
) -> Result<SqlInsertProcessor, CoreError> {
    refuse_lint("processor", conf, lint_insert)?;
    require(conf, &["driver", "dsn", "table", "columns", "args_mapping"])?;
    let obj = object(conf)?;
    let s = |k: &str| obj.get(k).and_then(JsonValue::as_str).map(String::from);
    let mapping = obj
        .get("args_mapping")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| CoreError::config("field args_mapping: expected a mapping string"))?;
    Ok(SqlInsertProcessor {
        db: LazyDb::new(Conn::from_parts(obj, "dsn", ctx.file_root.as_deref())?),
        table: s("table").unwrap_or_default(),
        columns: string_list(obj, "columns")?,
        prefix: s("prefix"),
        suffix: s("suffix"),
        args_mapping: ctx.blobl_env.compile_mapping(mapping)?,
    })
}

impl SqlInsertProcessor {
    async fn insert(&self, db: &Db, batch: &Batch) -> Result<(), String> {
        let mut args = Vec::new();
        let mut rows = Vec::with_capacity(batch.len());
        for i in 0..batch.len() {
            let row = batch_args(&self.args_mapping, batch, i).map_err(|e| e.to_string())?;
            rows.push(row.len());
            args.extend(row);
        }
        let sql = insert_sql(
            &self.table,
            &self.columns,
            &rows,
            self.prefix.as_deref(),
            self.suffix.as_deref(),
            Placeholder::for_driver(self.db.conn.driver),
        )?;
        util::exec(db, &sql, &args)
            .await
            .map_err(|e| util::db_error_text(&e))
    }
}

#[async_trait]
impl Processor for SqlInsertProcessor {
    fn processor_type(&self) -> &'static str {
        "sql_insert"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        mut batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        if batch.is_empty() {
            return Ok(Vec::new());
        }
        let db = self.db.get(&ctx.org).await?;
        for p in &mut batch {
            let _ = p.as_bytes();
        }
        if let Err(e) = self.insert(&db, &batch).await {
            for p in &mut batch {
                ctx.on_error(p, e.clone());
            }
        }
        Ok(vec![batch])
    }

    async fn close(&self) -> Result<(), CoreError> {
        self.db.close().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ekiben_core::{OrgId, OrgQuotas, Part, ResourceManager, StreamMetrics};
    use serde_json::json;

    fn ctx() -> ProcContext {
        ProcContext {
            stream_id: "s".into(),
            org: OrgContext::new(OrgId::new("o"), OrgQuotas::default()),
            resources: ResourceManager::new(),
            registry: Arc::new(Registry::new()),
            blobl_env: ekiben_bloblang::Environment::new(),
            metrics: StreamMetrics::new("s", "o"),
            label: String::new(),
            component_type: "sql".into(),
            path: "pipeline.processors.0".into(),
            file_root: None,
            shutdown_rx: tokio::sync::watch::channel(false).1,
        }
    }

    fn conf(kind: &str, v: JsonValue) -> ComponentConfig {
        ComponentConfig {
            label: String::new(),
            component_type: kind.into(),
            config: v,
            processors: Vec::new(),
        }
    }

    fn bodies(out: &[Batch]) -> Vec<String> {
        out[0]
            .iter()
            .map(|p| String::from_utf8_lossy(&p.as_bytes_ref()).into_owned())
            .collect()
    }

    const SEED: &str =
        "CREATE TABLE s (id integer, name text); INSERT INTO s VALUES (1, 'a'), (2, 'b')";

    #[tokio::test]
    async fn sql_raw_replaces_each_message_with_its_rows() {
        let ctx = ctx();
        let p = build_raw(
            &conf(
                "sql_raw",
                json!({"driver": "sqlite", "dsn": ":memory:", "init_statement": SEED,
                    "query": "SELECT name FROM s WHERE id = ?", "args_mapping": "root = [ this.id ]"}),
            ),
            &ctx,
        )
        .unwrap();
        let out = p
            .process_batch(
                &ctx,
                vec![Part::from_str(r#"{"id":2}"#), Part::from_str(r#"{"id":9}"#)],
            )
            .await
            .unwrap();
        assert_eq!(bodies(&out), [r#"[{"name":"b"}]"#, "[]"]);
        assert!(p.process_batch(&ctx, Vec::new()).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn sql_select_builds_its_query_and_a_failure_flags_the_message() {
        let ctx = ctx();
        let p = build_select(
            &conf(
                "sql_select",
                json!({"driver": "sqlite", "dsn": ":memory:", "init_statement": SEED,
                    "table": "s", "columns": ["name"], "where": "id = ?",
                    "args_mapping": "root = [ this.id ]"}),
            ),
            &ctx,
        )
        .unwrap();
        let out = p
            .process_batch(&ctx, vec![Part::from_str(r#"{"id":1}"#)])
            .await
            .unwrap();
        assert_eq!(bodies(&out), [r#"[{"name":"a"}]"#]);

        let bad = build_raw(
            &conf(
                "sql_raw",
                json!({"driver": "sqlite", "dsn": ":memory:", "query": "SELECT * FROM nope"}),
            ),
            &ctx,
        )
        .unwrap();
        let out = bad
            .process_batch(&ctx, vec![Part::from_str("{}")])
            .await
            .unwrap();
        assert!(out[0][0].is_errored());
        assert_eq!(bodies(&out), ["{}"], "a failed message keeps its content");
    }

    #[tokio::test]
    async fn sql_insert_writes_the_batch_and_keeps_the_messages() {
        let dir = tempfile::tempdir().unwrap();
        let dsn = dir.path().join("t.db").display().to_string();
        let ctx = ctx();
        let insert = build_insert(
            &conf(
                "sql_insert",
                json!({"driver": "sqlite", "dsn": dsn, "init_statement": "CREATE TABLE s (id integer, name text)",
                    "table": "s", "columns": ["id", "name"], "args_mapping": "root = [ this.id, this.name ]"}),
            ),
            &ctx,
        )
        .unwrap();
        let msgs = vec![
            Part::from_str(r#"{"id":1,"name":"a"}"#),
            Part::from_str(r#"{"id":2,"name":"b"}"#),
        ];
        let out = insert.process_batch(&ctx, msgs).await.unwrap();
        assert!(out[0].iter().all(|p| !p.is_errored()));
        assert_eq!(bodies(&out)[1], r#"{"id":2,"name":"b"}"#);
        insert.close().await.unwrap();

        let count = build_raw(
            &conf(
                "sql_raw",
                json!({"driver": "sqlite", "dsn": dsn, "query": "SELECT count(*) AS n FROM s"}),
            ),
            &ctx,
        )
        .unwrap();
        let out = count
            .process_batch(&ctx, vec![Part::from_str("{}")])
            .await
            .unwrap();
        assert_eq!(bodies(&out), [r#"[{"n":2}]"#]);
    }

    #[test]
    fn a_dynamic_query_is_refused() {
        let v = json!({"driver": "sqlite", "dsn": ":memory:", "query": "x", "unsafe_dynamic_query": true});
        assert!(lint_raw(&v)
            .iter()
            .any(|e| e.contains("unsafe_dynamic_query")));
        assert!(build_raw(&conf("sql_raw", v), &ctx()).is_err());
        assert!(build_insert(
            &conf(
                "sql_insert",
                json!({"driver": "sqlite", "dsn": ":memory:", "table": "t", "columns": ["a"]})
            ),
            &ctx()
        )
        .is_err());
    }
}
