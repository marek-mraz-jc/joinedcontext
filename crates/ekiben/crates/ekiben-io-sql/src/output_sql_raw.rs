//! Port of `internal/impl/sql/output_sql_raw.go` (Bento 1.22.0): runs `query` once per message
//! with the arguments of `args_mapping`. With `unsafe_dynamic_query` the query is an
//! interpolated string; it defaults to false, so a message never writes SQL text.

use async_trait::async_trait;
use ekiben_bloblang::{Executor, FieldExpression};
use ekiben_core::{
    lint_fields, Batch, ComponentConfig, CoreError, Output, PartMetaAdapter, PartsView, Registry,
    StreamContext,
};
use serde_json::Value as JsonValue;
use tokio::sync::RwLock;

use crate::conn_fields::{self, Db, Driver, CONN_FIELDS};
use crate::input_sql_raw::{refuse_lint, require, Conn};
use crate::util;

const FIELDS: &[&str] = &[
    "driver",
    "dsn",
    "query",
    "unsafe_dynamic_query",
    "args_mapping",
    "max_in_flight",
    "batching",
];

pub fn register(registry: &Registry) {
    registry.register_output("sql_raw", |conf, ctx| {
        Ok(std::sync::Arc::new(build(conf, ctx)?))
    });
    registry.register_lint("output", "sql_raw", lint);
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

pub(crate) fn lint_max_in_flight(obj: &serde_json::Map<String, JsonValue>) -> Option<String> {
    obj.get("max_in_flight")
        .filter(|v| v.as_i64().is_none())
        .map(|_| "field max_in_flight: expected an integer".to_string())
}

/// Evaluates `args_mapping` on message `i` of `batch` (`MessageBatchBloblangExecutor.Query`).
pub(crate) fn batch_args(
    exec: &Executor,
    batch: &Batch,
    i: usize,
) -> Result<Vec<ekiben_bloblang::Value>, CoreError> {
    let view = PartsView(batch);
    let mut scratch = ekiben_core::Part::default();
    let res = exec.map_part(i, &view, &mut PartMetaAdapter(&mut scratch), None)?;
    let v = res.ok_or_else(|| CoreError::other("mapping returned deleted message"))?;
    util::args_from_value(v).map_err(CoreError::other)
}

/// The connection half of an sql output: open lazily, closed and reported not-connected after a
/// bad connection or an authentication failure (`WriteBatch`).
pub(crate) struct OutConn {
    pub conn: Conn,
    pub db: RwLock<Option<Db>>,
}

impl OutConn {
    pub async fn connect(&self, ctx: &StreamContext) -> Result<(), CoreError> {
        let mut g = self.db.write().await;
        if g.is_none() {
            *g = Some(self.conn.open(ctx).await?);
        }
        Ok(())
    }

    pub async fn db(&self) -> Result<Db, CoreError> {
        self.db.read().await.clone().ok_or(CoreError::NotConnected)
    }

    /// Maps a driver error: a lost connection or failed login closes the pool and reports
    /// not-connected so the stream reconnects.
    pub async fn fail(&self, driver: Driver, e: crate::sqlx::Error) -> CoreError {
        if conn_fields::is_bad_conn(&e) || conn_fields::is_auth_error(driver, &e) {
            if let Some(db) = self.db.write().await.take() {
                db.close().await;
            }
            return CoreError::NotConnected;
        }
        CoreError::other(util::db_error_text(&e))
    }

    pub async fn close(&self) {
        if let Some(db) = self.db.write().await.take() {
            db.close().await;
        }
    }
}

pub struct SqlRawOutput {
    name: &'static str,
    out: OutConn,
    query_static: String,
    query_dyn: Option<FieldExpression>,
    args_mapping: Option<Executor>,
}

/// `newSQLRawOutput` over a config whose DSN sits under `dsn_key`.
pub(crate) fn new_raw_output(
    name: &'static str,
    obj: &serde_json::Map<String, JsonValue>,
    dsn_key: &str,
    unsafe_dyn: bool,
    ctx: &StreamContext,
) -> Result<SqlRawOutput, CoreError> {
    let conn = Conn::from_config(obj, dsn_key, ctx)?;
    let query_static = obj
        .get("query")
        .and_then(JsonValue::as_str)
        .unwrap_or_default()
        .to_string();
    let query_dyn = if unsafe_dyn {
        Some(ctx.blobl_env.compile_field(&query_static)?)
    } else {
        None
    };
    let args_mapping = match obj.get("args_mapping").and_then(JsonValue::as_str) {
        Some(m) => Some(ctx.blobl_env.compile_mapping(m)?),
        None => None,
    };
    Ok(SqlRawOutput {
        name,
        out: OutConn {
            conn,
            db: RwLock::new(None),
        },
        query_static,
        query_dyn,
        args_mapping,
    })
}

/// `newSQLRawOutputFromConfig`.
pub fn build(conf: &ComponentConfig, ctx: &StreamContext) -> Result<SqlRawOutput, CoreError> {
    refuse_lint("output", conf, lint)?;
    require(conf, &["driver", "dsn", "query"])?;
    let obj = conf
        .config
        .as_object()
        .ok_or_else(|| CoreError::config("sql_raw: expected an object"))?;
    let unsafe_dyn = obj
        .get("unsafe_dynamic_query")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false);
    new_raw_output("sql_raw", obj, "dsn", unsafe_dyn, ctx)
}

#[async_trait]
impl Output for SqlRawOutput {
    fn output_type(&self) -> &'static str {
        self.name
    }

    async fn connect(&self, ctx: &StreamContext) -> Result<(), CoreError> {
        self.out.connect(ctx).await
    }

    async fn write_batch(&self, _ctx: &StreamContext, mut batch: Batch) -> Result<(), CoreError> {
        let db = self.out.db().await?;
        for p in &mut batch {
            let _ = p.as_bytes();
        }
        for i in 0..batch.len() {
            let args = match &self.args_mapping {
                Some(exec) => batch_args(exec, &batch, i)?,
                None => Vec::new(),
            };
            let query = match &self.query_dyn {
                Some(expr) => expr
                    .resolve_string(i, &PartsView(&batch))
                    .map_err(|e| CoreError::other(format!("query interpolation error: {e}")))?,
                None => self.query_static.clone(),
            };
            if let Err(e) = util::exec(&db, &query, &args).await {
                return Err(self.out.fail(db.driver(), e).await);
            }
        }
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.out.db.try_read().map(|g| g.is_some()).unwrap_or(true)
    }

    async fn close(&self) -> Result<(), CoreError> {
        self.out.close().await;
        Ok(())
    }
}
