//! Port of `internal/impl/sql/input_sql_raw.go` (Bento 1.22.0): runs one query on connect and
//! emits a message per row; once the rows are exhausted the input ends. The row reader here is
//! shared with `sql_select`, whose Go code repeats it.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use ekiben_bloblang::Executor;
use ekiben_core::{
    lint_fields, AckResponder, Batch, ComponentConfig, CoreError, Input, Part, Registry,
    StreamContext,
};
use serde_json::Value as JsonValue;
use tokio::sync::{mpsc, Notify};

use crate::conn_fields::{self, ConnSettings, Db, Driver, Target, CONN_FIELDS};
use crate::util;

const FIELDS: &[&str] = &[
    "driver",
    "dsn",
    "query",
    "args_mapping",
    "auto_replay_nacks",
];

pub fn register(registry: &Registry) {
    registry.register_input("sql_raw", |conf, ctx| Ok(Box::new(build(conf, ctx)?)));
    registry.register_lint("input", "sql_raw", lint);
}

pub fn lint(conf: &JsonValue) -> Vec<String> {
    let known: Vec<&str> = FIELDS.iter().chain(CONN_FIELDS).copied().collect();
    let mut errs = lint_fields(conf, &known, &[]);
    if let Some(obj) = conf.as_object() {
        errs.extend(conn_fields::lint_conn(obj));
    }
    errs
}

/// Builds the connection half every sql component shares from its config object.
pub struct Conn {
    pub driver: Driver,
    pub target: Target,
    pub parse_time: bool,
    pub settings: ConnSettings,
}

impl Conn {
    pub fn from_config(
        obj: &serde_json::Map<String, JsonValue>,
        dsn_key: &str,
        ctx: &StreamContext,
    ) -> Result<Self, CoreError> {
        Self::from_parts(obj, dsn_key, ctx.file_root.as_deref())
    }

    /// As [`Self::from_config`] for a component built without a stream context (processors).
    pub fn from_parts(
        obj: &serde_json::Map<String, JsonValue>,
        dsn_key: &str,
        file_root: Option<&std::path::Path>,
    ) -> Result<Self, CoreError> {
        let driver = Driver::parse(
            obj.get("driver")
                .and_then(JsonValue::as_str)
                .unwrap_or_default(),
        )
        .map_err(CoreError::config)?;
        let dsn = obj
            .get(dsn_key)
            .and_then(JsonValue::as_str)
            .ok_or_else(|| CoreError::config(format!("field {dsn_key} is required")))?;
        let (target, parse_time) = conn_fields::parse_dsn(driver, dsn, file_root)?;
        Ok(Self {
            driver,
            target,
            parse_time,
            settings: ConnSettings::from_config(obj, file_root)?,
        })
    }

    pub async fn open(&self, ctx: &StreamContext) -> Result<Db, CoreError> {
        self.open_for(&ctx.org).await
    }

    /// Opens the pool for `org`: its outbound rule judges the DSN's host first.
    pub async fn open_for(&self, org: &Arc<ekiben_core::OrgContext>) -> Result<Db, CoreError> {
        let db = conn_fields::open(&self.target, &self.settings, org).await?;
        self.settings.apply(&db).await;
        Ok(db)
    }
}

/// Lints `conf` with `lint` and fails the build on the first error, so `ekiben run` refuses what
/// `ekiben lint` refuses.
pub(crate) fn refuse_lint(
    kind: &str,
    conf: &ComponentConfig,
    lint: fn(&JsonValue) -> Vec<String>,
) -> Result<(), CoreError> {
    match lint(&conf.config).into_iter().next() {
        Some(e) => Err(CoreError::config(format!(
            "{kind} {}: {e}",
            conf.component_type
        ))),
        None => Ok(()),
    }
}

/// Bento checks required fields when it parses a config, not at lint; gives the config object.
pub(crate) fn require<'a>(
    conf: &'a ComponentConfig,
    required: &[&str],
) -> Result<&'a serde_json::Map<String, JsonValue>, CoreError> {
    let err = |e: String| CoreError::config(format!("{} {e}", conf.component_type));
    let obj = conf
        .config
        .as_object()
        .ok_or_else(|| err("expected object value".into()))?;
    match required.iter().find(|k| !obj.contains_key(**k)) {
        Some(k) => Err(err(format!("field {k} is required"))),
        None => Ok(obj),
    }
}

/// Bento's `AutoRetryNacks`: a nacked row is read again; the input ends only once every row is acked.
#[derive(Default)]
struct Pending {
    replay: Mutex<VecDeque<Batch>>,
    in_flight: Mutex<usize>,
    changed: Notify,
}

/// The row reader of `sql_raw` and `sql_select`.
pub struct RowsInput {
    name: &'static str,
    conn: Conn,
    query: String,
    args_mapping: Option<Executor>,
    auto_replay_nacks: bool,
    db: Option<Db>,
    rows: Option<mpsc::Receiver<Result<ekiben_bloblang::Value, String>>>,
    pending: Arc<Pending>,
}

impl RowsInput {
    pub fn new(
        name: &'static str,
        conn: Conn,
        query: String,
        obj: &serde_json::Map<String, JsonValue>,
        ctx: &StreamContext,
    ) -> Result<Self, CoreError> {
        let args_mapping = match obj.get("args_mapping").and_then(JsonValue::as_str) {
            Some(m) => Some(ctx.blobl_env.compile_mapping(m)?),
            None => None,
        };
        Ok(Self {
            name,
            conn,
            query,
            args_mapping,
            auto_replay_nacks: obj
                .get("auto_replay_nacks")
                .and_then(JsonValue::as_bool)
                .unwrap_or(true),
            db: None,
            rows: None,
            pending: Arc::default(),
        })
    }

    fn ack_for(&self, batch: &Batch) -> AckResponder {
        if !self.auto_replay_nacks {
            return AckResponder::noop();
        }
        let pending = self.pending.clone();
        *pending
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner) += 1;
        let copy: Batch = batch.iter().map(Part::deep_copy).collect();
        AckResponder::from_fn(move |res| {
            if res.is_some() {
                pending
                    .replay
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push_back(copy);
            }
            *pending
                .in_flight
                .lock()
                .unwrap_or_else(PoisonError::into_inner) -= 1;
            pending.changed.notify_waiters();
        })
    }

    fn pop_replay(&self) -> Option<Batch> {
        self.pending
            .replay
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
    }
}

#[async_trait]
impl Input for RowsInput {
    fn input_type(&self) -> &'static str {
        self.name
    }

    async fn connect(&mut self, ctx: &StreamContext) -> Result<(), CoreError> {
        if self.db.is_some() {
            return Ok(());
        }
        let args = match &self.args_mapping {
            Some(exec) => {
                let v = exec.exec_on_value(ekiben_bloblang::Value::Null)?;
                util::args_from_value(v).map_err(CoreError::other)?
            }
            None => Vec::new(),
        };
        let db = self.conn.open(ctx).await?;
        let (tx, rx) = mpsc::channel(16);
        tokio::spawn(util::stream_rows(
            db.clone(),
            self.query.clone(),
            args,
            self.conn.parse_time,
            tx,
        ));
        self.db = Some(db);
        self.rows = Some(rx);
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.db.is_some()
    }

    async fn read_batch(
        &mut self,
        _ctx: &StreamContext,
    ) -> Result<(Batch, AckResponder), CoreError> {
        if let Some(b) = self.pop_replay() {
            let ack = self.ack_for(&b);
            return Ok((b, ack));
        }
        if self.db.is_none() && self.rows.is_none() {
            return Err(CoreError::NotConnected);
        }
        let next = match self.rows.as_mut() {
            Some(rx) => rx.recv().await,
            None => None,
        };
        match next {
            Some(Ok(row)) => {
                let batch = vec![Part::from_structured(row)];
                let ack = self.ack_for(&batch);
                Ok((batch, ack))
            }
            Some(Err(e)) => {
                self.rows = None;
                Err(CoreError::other(e))
            }
            None => {
                self.rows = None;
                // AutoRetryNacks: end only once nothing is in flight or waiting for a replay.
                loop {
                    let notified = self.pending.changed.notified();
                    if let Some(b) = self.pop_replay() {
                        let ack = self.ack_for(&b);
                        return Ok((b, ack));
                    }
                    if *self
                        .pending
                        .in_flight
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        == 0
                    {
                        return Err(CoreError::EndOfInput);
                    }
                    notified.await;
                }
            }
        }
    }

    async fn close(&mut self) -> Result<(), CoreError> {
        self.rows = None;
        if let Some(db) = self.db.take() {
            db.close().await;
        }
        Ok(())
    }
}

/// `newSQLRawInputFromConfig`.
pub fn build(conf: &ComponentConfig, ctx: &StreamContext) -> Result<RowsInput, CoreError> {
    refuse_lint("input", conf, lint)?;
    let obj = require(conf, &["driver", "dsn", "query"])?;
    let conn = Conn::from_config(obj, "dsn", ctx)?;
    let query = obj
        .get("query")
        .and_then(JsonValue::as_str)
        .unwrap_or_default()
        .to_string();
    RowsInput::new("sql_raw", conn, query, obj, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{conf, stream_ctx};
    use ekiben_core::BatchError;
    use serde_json::json;
    use std::time::Duration;

    /// A file database holding `t (a)` with the rows 1, 2 and 3.
    fn seeded() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let dsn = dir.path().join("in.db").display().to_string();
        rusqlite::Connection::open(&dsn)
            .unwrap()
            .execute_batch("CREATE TABLE t (a int); INSERT INTO t VALUES (1), (2), (3)")
            .unwrap();
        (dir, dsn)
    }

    fn body(b: &Batch) -> String {
        String::from_utf8_lossy(&b[0].as_bytes_ref()).into_owned()
    }

    #[test]
    fn require_names_the_component_and_the_missing_field() {
        let e = require(&conf("sql_raw", json!("x")), &["dsn"]).unwrap_err();
        assert_eq!(e.to_string(), CoreError::config("sql_raw expected object value").to_string());
        let e = require(&conf("sql_raw", json!({"driver": "sqlite"})), &["driver", "dsn"])
            .unwrap_err();
        assert!(e.to_string().contains("sql_raw field dsn is required"), "{e}");
        let e = build(&conf("sql_raw", json!({"driver": "sqlite", "dsn": ":memory:",
            "query": "x", "nope": 1})), &stream_ctx(None))
        .err()
        .expect("refused");
        assert!(e.to_string().contains("input sql_raw: field nope not recognised"), "{e}");
        let e = build(&conf("sql_raw", json!({"driver": "sqlite", "dsn": ":memory:",
            "query": "x", "args_mapping": "root = ("})), &stream_ctx(None))
        .err()
        .expect("refused");
        assert!(!e.to_string().is_empty());
        let e = build(&conf("sql_raw", json!({"driver": "sqlite", "dsn": 5, "query": "x"})), &stream_ctx(None))
            .err()
            .expect("refused");
        assert!(e.to_string().contains("field dsn is required"), "{e}");
    }

    #[tokio::test]
    async fn a_nacked_row_is_read_again_and_the_input_ends_once_all_are_acked() {
        let (_dir, dsn) = seeded();
        let ctx = stream_ctx(None);
        let mut input = build(
            &conf("sql_raw", json!({"driver": "sqlite", "dsn": dsn,
                "query": "SELECT a FROM t WHERE a >= ? ORDER BY a", "args_mapping": "root = [2]"})),
            &ctx,
        )
        .unwrap();
        assert!(matches!(input.read_batch(&ctx).await, Err(CoreError::NotConnected)));
        assert!(!input.is_connected());
        input.connect(&ctx).await.unwrap();
        input.connect(&ctx).await.unwrap();
        assert!(input.is_connected());
        let (first, ack1) = input.read_batch(&ctx).await.unwrap();
        assert_eq!(body(&first), r#"{"a":2}"#);
        ack1.nack(BatchError::new(first, "write failed"));
        let (again, ack) = input.read_batch(&ctx).await.unwrap();
        assert_eq!(body(&again), r#"{"a":2}"#, "the nacked row comes back first");
        ack.ack();
        let (third, ack3) = input.read_batch(&ctx).await.unwrap();
        assert_eq!(body(&third), r#"{"a":3}"#);
        // The rows are spent but one is in flight: the input waits for its verdict.
        let late = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            ack3.nack(BatchError::new(Vec::new(), "late"));
        });
        let (replayed, ack) = input.read_batch(&ctx).await.unwrap();
        assert_eq!(body(&replayed), r#"{"a":3}"#);
        late.await.unwrap();
        ack.ack();
        assert!(matches!(input.read_batch(&ctx).await, Err(CoreError::EndOfInput)));
        input.close().await.unwrap();
        input.close().await.unwrap();
        assert!(!input.is_connected());
    }

    #[tokio::test]
    async fn without_replays_the_input_ends_after_the_last_row_and_passes_query_errors() {
        let (_dir, dsn) = seeded();
        let ctx = stream_ctx(None);
        let mut input = build(
            &conf("sql_raw", json!({"driver": "sqlite", "dsn": dsn, "auto_replay_nacks": false,
                "query": "SELECT a FROM t ORDER BY a LIMIT 1"})),
            &ctx,
        )
        .unwrap();
        input.connect(&ctx).await.unwrap();
        let (b, ack) = input.read_batch(&ctx).await.unwrap();
        ack.nack(BatchError::new(b, "dropped"));
        assert!(matches!(input.read_batch(&ctx).await, Err(CoreError::EndOfInput)));

        let mut bad = build(
            &conf("sql_raw", json!({"driver": "sqlite", "dsn": dsn, "query": "SELECT nope FROM t"})),
            &ctx,
        )
        .unwrap();
        bad.connect(&ctx).await.unwrap();
        let e = bad.read_batch(&ctx).await.err().expect("a query error");
        assert!(e.to_string().contains("no such column"), "{e}");
        assert!(matches!(bad.read_batch(&ctx).await, Err(CoreError::EndOfInput)));
        // An args_mapping that does not give an array fails the connect.
        let mut odd = build(
            &conf("sql_raw", json!({"driver": "sqlite", "dsn": dsn, "query": "SELECT 1",
                "args_mapping": "root = \"x\""})),
            &ctx,
        )
        .unwrap();
        let e = odd.connect(&ctx).await.unwrap_err();
        assert!(e.to_string().contains("non-array result"), "{e}");
    }
}
