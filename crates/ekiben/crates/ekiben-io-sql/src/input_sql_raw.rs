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
        let (target, parse_time) = conn_fields::parse_dsn(driver, dsn, ctx.file_root.as_deref())?;
        Ok(Self {
            driver,
            target,
            parse_time,
            settings: ConnSettings::from_config(obj, ctx.file_root.as_deref())?,
        })
    }

    pub async fn open(&self, ctx: &StreamContext) -> Result<Db, CoreError> {
        let db = conn_fields::open(&self.target, &self.settings, &ctx.org).await?;
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

/// Bento checks required fields when it parses a config, not at lint.
pub(crate) fn require(conf: &ComponentConfig, required: &[&str]) -> Result<(), CoreError> {
    match lint_fields(&conf.config, required, required)
        .into_iter()
        .find(|e| e.ends_with("is required"))
    {
        Some(e) => Err(CoreError::config(format!("{} {e}", conf.component_type))),
        None => Ok(()),
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
    require(conf, &["driver", "dsn", "query"])?;
    let obj = conf
        .config
        .as_object()
        .ok_or_else(|| CoreError::config("sql_raw: expected an object"))?;
    let conn = Conn::from_config(obj, "dsn", ctx)?;
    let query = obj
        .get("query")
        .and_then(JsonValue::as_str)
        .unwrap_or_default()
        .to_string();
    RowsInput::new("sql_raw", conn, query, obj, ctx)
}
