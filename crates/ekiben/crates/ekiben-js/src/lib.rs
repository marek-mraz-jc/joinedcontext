#![deny(unsafe_code)]
//! `javascript` processor (Bento 1.22.0 `internal/impl/javascript`) on QuickJS through rquickjs.
//!
//! The script sees one message through Bento's v0 API: `bento.v0_msg_as_string`,
//! `v0_msg_set_string`, `v0_msg_as_structured`, `v0_msg_set_structured`, `v0_msg_get_meta`,
//! `v0_msg_set_meta`, `v0_msg_exists_meta`, and `console.log` (`info`, `warn`, `error`), which
//! writes to the runner's log.
//!
//! Isolation, per message: a fresh Context (no state passes from one message to the next) in a
//! Runtime of the batch, with a memory limit (`memory_mb`, default 32 MiB, at most 64), a 256 KiB
//! stack and an interrupt deadline (`timeout_ms`, default 50, at most 200). The script reaches no
//! network, file, environment, module loader or timer, and no promise job ever runs: the context carries none of them,
//! `eval` and the `Function` constructors are removed, and `bento.v0_fetch` does not exist. A
//! message whose script throws, runs out of time or out of memory is flagged with the reason and
//! the batch goes on. Scripts run on the blocking pool, never on the async workers.
//!
//! ekiben deviations: `file` and `global_folders` are refused (code is inline, nothing is
//! `require`d), and the two limits are ekiben fields. jc-core refuses `javascript` as an author
//! step; it reaches the runner only through a reviewed Script (T-7061 G-25).
//!
//! Script time counts against the organization's `max_cpu_time_ms_per_run`: the runner charges
//! the batch's time to the run (`StreamMetrics::charge_processing`), and inside a batch no script
//! runs past what the run has left, so the messages after the budget is spent are flagged unrun.
//! ponytail: workers running batches in parallel each see the budget left when their batch
//! started, so a run overshoots by at most one budget per worker. At most a quarter of the
//! process's cores run scripts at once ([`script_slots`]); a batch waiting for a slot is charged
//! for the wait as for any time inside a processor.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use ekiben_bloblang::Value;
use ekiben_core::{
    lint_fields, Batch, ComponentConfig, CoreError, Part, ProcContext, Processor, Registry,
};
use rquickjs::context::intrinsic;
use rquickjs::{Context, Ctx, Function, Runtime};
use serde_json::Value as JsonValue;

pub const DEFAULT_TIMEOUT_MS: u64 = 50;
pub const MAX_TIMEOUT_MS: u64 = 200;
pub const DEFAULT_MEMORY_MB: u64 = 32;
pub const MAX_MEMORY_MB: u64 = 64;
const STACK_BYTES: usize = 256 * 1024;
/// Lines one message may write through `console`, and the bytes of each.
const MAX_LOG_LINES: usize = 100;
const MAX_LOG_LINE: usize = 1024;

const FIELDS: &[&str] = &["code", "timeout_ms", "memory_mb"];

/// Turns the natives into `bento` and `console`, then removes every way to compile a string.
const PRELUDE: &str = r#""use strict";
(() => {
  const n = {};
  for (const k of ["as_string", "as_json", "set_string", "set_json", "get_meta", "set_meta", "log"]) {
    n[k] = globalThis["__ekiben_" + k];
    delete globalThis["__ekiben_" + k];
  }
  const meta = (k) => n.get_meta(String(k));
  Object.defineProperty(globalThis, "bento", { value: Object.freeze({
    v0_msg_as_string: () => n.as_string(),
    v0_msg_set_string: (v) => { n.set_string(String(v)); },
    v0_msg_as_structured: () => {
      const j = n.as_json();
      if (j == null) throw new Error("failed to parse message as JSON");
      return JSON.parse(j);
    },
    v0_msg_set_structured: (v) => { n.set_json(JSON.stringify(v === undefined ? null : v)); },
    v0_msg_exists_meta: (k) => meta(k) != null,
    v0_msg_get_meta: (k) => {
      const j = meta(k);
      if (j == null) throw new Error("key not found");
      return JSON.parse(j);
    },
    v0_msg_set_meta: (k, v) => { n.set_meta(String(k), JSON.stringify(v === undefined ? null : v)); },
  }) });
  const line = (level) => (...a) => n.log(level, a.map((x) => typeof x === "string" ? x : JSON.stringify(x)).join(" "));
  Object.defineProperty(globalThis, "console", { value: Object.freeze({
    log: line("info"), info: line("info"), warn: line("warn"), error: line("error"),
  }) });
  for (const f of [function () {}, function* () {}, async function () {}, async function* () {}]) {
    Object.defineProperty(Object.getPrototypeOf(f), "constructor", { value: undefined });
  }
  delete globalThis.eval;
  delete globalThis.Function;
})();
"#;

pub fn register(registry: &Registry) {
    registry.register_processor("javascript", |conf, _ctx| {
        Ok(Arc::new(JavascriptProcessor::from_config(conf)?))
    });
    registry.register_lint("processor", "javascript", lint);
}

fn limit(conf: &JsonValue, field: &str, default: u64, max: u64) -> Result<u64, String> {
    match conf.get(field) {
        None | Some(JsonValue::Null) => Ok(default),
        Some(v) => v
            .as_u64()
            .filter(|n| (1..=max).contains(n))
            .ok_or_else(|| format!("field {field}: expected a whole number from 1 to {max}")),
    }
}

pub fn lint(conf: &JsonValue) -> Vec<String> {
    let mut errs = Vec::new();
    if conf.get("file").is_some() {
        errs.push("field file is not supported by ekiben: give the script as code".to_string());
    }
    if conf
        .get("global_folders")
        .and_then(JsonValue::as_array)
        .is_some_and(|a| !a.is_empty())
    {
        errs.push(
            "field global_folders is not supported by ekiben: a script requires no module"
                .to_string(),
        );
    }
    let mut known = FIELDS.to_vec();
    known.extend(["file", "global_folders"]);
    errs.extend(lint_fields(conf, &known, &["code"]));
    if conf.get("code").is_some_and(|c| !c.is_string()) {
        errs.push("field code: expected a string".to_string());
    }
    for (field, default, max) in [
        ("timeout_ms", DEFAULT_TIMEOUT_MS, MAX_TIMEOUT_MS),
        ("memory_mb", DEFAULT_MEMORY_MB, MAX_MEMORY_MB),
    ] {
        if let Err(e) = limit(conf, field, default, max) {
            errs.push(e);
        }
    }
    errs
}

#[derive(Clone, Copy, Debug)]
struct Limits {
    timeout: Duration,
    memory_bytes: usize,
}

pub struct JavascriptProcessor {
    code: Arc<str>,
    limits: Limits,
    label: String,
}

impl JavascriptProcessor {
    pub fn from_config(conf: &ComponentConfig) -> Result<Self, CoreError> {
        let errs = lint(&conf.config);
        if !errs.is_empty() {
            return Err(CoreError::config(format!(
                "javascript: {}",
                errs.join("; ")
            )));
        }
        let code = conf
            .config
            .get("code")
            .and_then(JsonValue::as_str)
            .unwrap_or_default();
        let timeout = limit(
            &conf.config,
            "timeout_ms",
            DEFAULT_TIMEOUT_MS,
            MAX_TIMEOUT_MS,
        )
        .map_err(CoreError::config)?;
        let memory = limit(&conf.config, "memory_mb", DEFAULT_MEMORY_MB, MAX_MEMORY_MB)
            .map_err(CoreError::config)?;
        Ok(Self {
            code: Arc::from(code),
            limits: Limits {
                timeout: Duration::from_millis(timeout),
                memory_bytes: usize::try_from(memory << 20).unwrap_or(usize::MAX),
            },
            label: conf.label.clone(),
        })
    }
}

/// One message's script run: the message, and the lines it logged.
struct Run {
    part: Part,
    logs: Vec<(String, String)>,
}

/// What a script did wrong, in words the author can act on.
fn reason(message: &str, limits: Limits) -> String {
    if message.contains("interrupted") {
        format!(
            "javascript: the script ran past its limit of {} ms",
            limits.timeout.as_millis()
        )
    } else if message.contains("out of memory") {
        format!(
            "javascript: the script used more than its limit of {} MiB",
            limits.memory_bytes >> 20
        )
    } else {
        format!("javascript: {message}")
    }
}

fn exception_text(ctx: &Ctx<'_>, err: rquickjs::Error) -> String {
    if !err.is_exception() {
        return err.to_string();
    }
    let thrown = ctx.catch();
    if let Some(ex) = thrown.as_exception() {
        return ex.message().unwrap_or_else(|| "an exception".to_string());
    }
    thrown
        .as_string()
        .and_then(|s| s.to_string().ok())
        .unwrap_or_else(|| "the script threw a value that is not an Error".to_string())
}

fn install<'js>(ctx: &Ctx<'js>, run: &Rc<RefCell<Run>>) -> rquickjs::Result<()> {
    let g = ctx.globals();
    let r = run.clone();
    g.set(
        "__ekiben_as_string",
        Function::new(ctx.clone(), move || {
            String::from_utf8_lossy(&r.borrow().part.as_bytes_ref()).into_owned()
        })?,
    )?;
    let r = run.clone();
    g.set(
        "__ekiben_as_json",
        Function::new(ctx.clone(), move || -> Option<String> {
            let part = &r.borrow().part;
            if part.holds_structured() {
                let v = part.as_structured_ref().ok()?;
                return serde_json::to_string(&v.to_json_value()).ok();
            }
            let bytes = part.as_bytes_ref();
            serde_json::from_slice::<JsonValue>(&bytes).ok()?;
            Some(String::from_utf8_lossy(&bytes).into_owned())
        })?,
    )?;
    let r = run.clone();
    g.set(
        "__ekiben_set_string",
        Function::new(ctx.clone(), move |s: String| {
            r.borrow_mut().part.set_bytes(s.into_bytes());
        })?,
    )?;
    let r = run.clone();
    g.set(
        "__ekiben_set_json",
        Function::new(ctx.clone(), move |s: String| {
            if let Ok(v) = serde_json::from_str::<JsonValue>(&s) {
                r.borrow_mut()
                    .part
                    .set_structured(Value::from_json_value(v));
            }
        })?,
    )?;
    let r = run.clone();
    g.set(
        "__ekiben_get_meta",
        Function::new(ctx.clone(), move |k: String| -> Option<String> {
            let run = r.borrow();
            let v = run.part.meta_get(&k)?;
            serde_json::to_string(&v.to_json_value()).ok()
        })?,
    )?;
    let r = run.clone();
    g.set(
        "__ekiben_set_meta",
        Function::new(ctx.clone(), move |k: String, s: String| {
            if let Ok(v) = serde_json::from_str::<JsonValue>(&s) {
                r.borrow_mut().part.meta_set(k, Value::from_json_value(v));
            }
        })?,
    )?;
    let r = run.clone();
    g.set(
        "__ekiben_log",
        Function::new(ctx.clone(), move |level: String, mut line: String| {
            let logs = &mut r.borrow_mut().logs;
            if logs.len() < MAX_LOG_LINES {
                if line.len() > MAX_LOG_LINE {
                    let mut cut = MAX_LOG_LINE;
                    while !line.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    line.truncate(cut);
                }
                logs.push((level, line));
            }
        })?,
    )?;
    Ok(())
}

fn new_runtime(limits: Limits, deadline: &Rc<Cell<Instant>>) -> Result<Runtime, String> {
    let rt = Runtime::new().map_err(|e| format!("javascript: starting the engine: {e}"))?;
    rt.set_memory_limit(limits.memory_bytes);
    rt.set_max_stack_size(STACK_BYTES);
    let d = deadline.clone();
    rt.set_interrupt_handler(Some(Box::new(move || Instant::now() >= d.get())));
    Ok(rt)
}

/// Runs `code` on one message in a fresh context of `rt`.
fn run_one(rt: &Runtime, code: &str, part: Part, limits: Limits) -> (Run, Result<(), String>) {
    let run = Rc::new(RefCell::new(Run {
        part,
        logs: Vec::new(),
    }));
    let outcome = Context::builder()
        .with::<intrinsic::Eval>()
        .with::<intrinsic::Json>()
        .with::<intrinsic::Date>()
        .with::<intrinsic::RegExpCompiler>()
        .with::<intrinsic::RegExp>()
        .with::<intrinsic::MapSet>()
        .with::<intrinsic::TypedArrays>()
        // Without Promise an async function leaves objects QuickJS asserts on when the runtime
        // is freed (an abort of the runner); with it, a promise's jobs are simply never run.
        .with::<intrinsic::Promise>()
        .build(rt)
        .map_err(|e| reason(&e.to_string(), limits))
        .and_then(|context| {
            context.with(|ctx| {
                install(&ctx, &run).map_err(|e| reason(&exception_text(&ctx, e), limits))?;
                ctx.eval::<(), _>(PRELUDE)
                    .map_err(|e| reason(&exception_text(&ctx, e), limits))?;
                ctx.eval::<(), _>(code)
                    .map_err(|e| reason(&exception_text(&ctx, e), limits))
            })
        });
    rt.run_gc();
    let run = Rc::try_unwrap(run).map_or_else(
        // A closure still held: the context is gone, so this is unreachable in practice; the
        // message keeps its state as last seen.
        |rc| {
            let mut r = rc.borrow_mut();
            Run {
                part: std::mem::take(&mut r.part),
                logs: std::mem::take(&mut r.logs),
            }
        },
        RefCell::into_inner,
    );
    (run, outcome)
}

fn budget_spent(budget: Duration) -> String {
    format!(
        "javascript: the run's processing time budget (max_cpu_time_ms_per_run) is spent; {} ms \
         were left for this batch",
        budget.as_millis()
    )
}

/// Scripts that may run at once in this process: a quarter of the cores, at least one. A
/// stream's workers run its batches in parallel, so without this one stream spinning every
/// script to its deadline holds a core per worker; js_isolation measured the other streams' p99
/// going from 1.2 ms to 9-13 ms with eight spinning on eight workers (T-7082).
fn script_slots() -> &'static tokio::sync::Semaphore {
    static SLOTS: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    SLOTS.get_or_init(|| {
        let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
        tokio::sync::Semaphore::new((cores / 4).max(1))
    })
}

/// The batch after the script ran on each message: the flags to set and the lines to log.
type Outcome = (Batch, Vec<(usize, String)>, Vec<(String, String)>);

/// Runs `code` over each message of `batch`, spending at most `budget` in total.
fn run_batch(code: &str, limits: Limits, budget: Duration, mut batch: Batch) -> Outcome {
    let started = Instant::now();
    let deadline = Rc::new(Cell::new(started));
    let mut errors = Vec::new();
    let mut logs = Vec::new();
    let mut rt = None;
    for (i, slot) in batch.iter_mut().enumerate() {
        if rt.is_none() {
            match new_runtime(limits, &deadline) {
                Ok(r) => rt = Some(r),
                Err(e) => {
                    errors.push((i, e));
                    continue;
                }
            }
        }
        let Some(runtime) = rt.as_ref() else {
            continue;
        };
        let left = budget.saturating_sub(started.elapsed());
        if left.is_zero() {
            errors.push((i, budget_spent(budget)));
            continue;
        }
        deadline.set(Instant::now() + limits.timeout.min(left));
        let (run, outcome) = run_one(runtime, code, std::mem::take(slot), limits);
        *slot = run.part;
        logs.extend(run.logs);
        if let Err(mut e) = outcome {
            if e.contains("ran past") && left < limits.timeout && started.elapsed() >= budget {
                e = budget_spent(budget);
            }
            // A runtime that ran out of memory is not trusted with the next message.
            if e.contains("MiB") {
                rt = None;
            }
            errors.push((i, e));
        }
    }
    (batch, errors, logs)
}

#[async_trait]
impl Processor for JavascriptProcessor {
    fn processor_type(&self) -> &'static str {
        "javascript"
    }

    fn label(&self) -> &str {
        &self.label
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        let (code, limits) = (self.code.clone(), self.limits);
        let limit_ns = ctx
            .org
            .get_quotas()
            .max_cpu_time_ms_per_run
            .saturating_mul(1_000_000);
        let budget = Duration::from_nanos(limit_ns.saturating_sub(ctx.metrics.processing_ns()));
        let _slot = script_slots()
            .acquire()
            .await
            .map_err(|e| CoreError::other(format!("javascript: {e}")))?;
        let (mut batch, errors, logs) =
            tokio::task::spawn_blocking(move || run_batch(&code, limits, budget, batch))
                .await
                .map_err(|e| {
                    CoreError::other(format!("javascript: the script task failed: {e}"))
                })?;
        for (level, line) in logs {
            match level.as_str() {
                "error" => tracing::error!(stream = %ctx.stream_id, label = %self.label, "{line}"),
                "warn" => tracing::warn!(stream = %ctx.stream_id, label = %self.label, "{line}"),
                _ => tracing::info!(stream = %ctx.stream_id, label = %self.label, "{line}"),
            }
        }
        for (i, e) in errors {
            if let Some(part) = batch.get_mut(i) {
                ctx.on_error(part, e);
            }
        }
        Ok(vec![batch])
    }
}

/// The sandbox without a stream, for the fuzz targets in `fuzz/`.
#[cfg(feature = "fuzzing")]
pub mod fuzzing {
    use super::*;

    /// Runs `code` over two copies of `message` in one runtime (the second sees what the first
    /// left in it), with the default limits; returns each message's flag.
    pub fn run(code: &str, message: &[u8]) -> Vec<(usize, String)> {
        let limits = Limits {
            timeout: Duration::from_millis(DEFAULT_TIMEOUT_MS),
            memory_bytes: usize::try_from(DEFAULT_MEMORY_MB << 20).unwrap_or(usize::MAX),
        };
        let batch = vec![Part::new(message.to_vec()), Part::new(message.to_vec())];
        run_batch(code, limits, Duration::from_secs(1), batch).1
    }
}

#[cfg(test)]
mod tests;
