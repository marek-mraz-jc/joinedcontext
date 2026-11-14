//! The `jq` processor (`internal/impl/pure/processor_jq.go`, Bento 1.22.0).
//!
//! Bento runs the query with gojq; ekiben runs it with jaq, with the values converted the way gojq
//! sees them: JSON numbers as integers when they fit, else floats; objects with sorted keys;
//! `$metadata` the message's metadata; `$ENV` empty (gojq has no environment without a loader).
//! A query that emits nothing drops the message, several outputs become one array, `output_raw`
//! writes strings bare and everything else as Go's `json.Marshal` does, concatenated.
//!
//! Limits gojq lacks, so one config cannot exhaust the process: a query of at most
//! [`MAX_QUERY_BYTES`], at most [`MAX_OUTPUTS`] values per message and [`MAX_OUTPUT_BYTES`] of raw
//! output; per message at most [`MAX_STEPS`] evaluation steps, [`MAX_TIME`], [`MAX_EVAL_STACK`]
//! of stack and [`MAX_BUILT_BYTES`] of strings, array slots and JSON text built (jc's jaq-core and
//! jaq-json budget patch, vendor/SOURCES.md). A batch runs on a thread of its own
//! with a [`THREAD_STACK`] stack, off the async workers, so a deep recursion (`def f: 1 + f; f`) or
//! an endless loop (`last(range(1e15))`) fails its message and leaves the process running.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ekiben_bloblang::Value;
use ekiben_core::{Batch, ComponentConfig, CoreError, Part, ProcContext, Processor, Registry};
use jaq_core::data::JustLut;
use jaq_core::load::{Arena, File, Loader};
use jaq_core::{Compiler, Ctx, Native, Vars};
use jaq_json::{Num, Val};

use crate::fields::Fields;

pub const MAX_QUERY_BYTES: usize = 64 * 1024;
pub const MAX_OUTPUTS: usize = 100_000;
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_STEPS: u64 = 10_000_000;
pub const MAX_TIME: Duration = Duration::from_secs(5);
pub const MAX_EVAL_STACK: usize = 64 << 20;
pub const MAX_BUILT_BYTES: usize = 256 << 20;
/// An error value is printed into the message's error at most this long.
const MAX_ERROR_BYTES: usize = 4096;
/// The evaluation thread's stack: [`MAX_EVAL_STACK`] for jaq, the rest for converting and dropping
/// the values a query built (both recurse over their nesting). Reserved, touched only as used.
pub const THREAD_STACK: usize = 1 << 30;
/// Deepest result a message may carry: the stream serializes it later on an async worker's stack.
/// Go's `encoding/json` reads at most 10000 levels; Bento writes any depth.
pub const MAX_DEPTH: usize = 1000;

/// gojq built-ins jaq lacks or defines otherwise: `env` (empty, as gojq without an environ
/// loader), `@json`, and gojq's `@csv` and `@tsv` (`funcToCSV`, `funcToTSV`).
const GOJQ_DEFS: &str = r#"
def env: $ENV;
def @json: tojson;
def @csv: if type == "array" then map(
    if type == "string" then "\"" + (split("\"") | join("\"\"")) + "\""
    elif type == "number" or type == "boolean" then tojson
    elif . == null then ""
    else error("invalid csv field value: \(tojson)") end) | join(",")
  else error("cannot csv format: \(type)") end;
def @tsv: if type == "array" then map(
    if type == "string" then gsub("\\\\"; "\\\\") | gsub("\t"; "\\t") | gsub("\r"; "\\r") | gsub("\n"; "\\n")
    elif type == "number" or type == "boolean" then tojson
    elif . == null then ""
    else error("invalid tsv field value: \(tojson)") end) | join("\t")
  else error("cannot tsv format: \(type)") end;
"#;

type JqFilter = jaq_core::compile::Filter<Native<JustLut<Val>>>;

pub fn register(registry: &Registry) {
    registry.register_processor("jq", |conf, _ctx| {
        Ok(Arc::new(JqProcessor::from_config(conf)?))
    });
}

/// Implements `jq`.
pub struct JqProcessor {
    filter: Arc<JqFilter>,
    in_raw: bool,
    out_raw: bool,
}

impl JqProcessor {
    pub fn from_config(conf: &ComponentConfig) -> Result<Self, CoreError> {
        let f = Fields::new("jq", &conf.config, &["query", "raw", "output_raw"])?;
        let query = f.string("query")?.ok_or_else(|| f.missing("query"))?;
        let in_raw = f.bool("raw")?.unwrap_or(false);
        let out_raw = f.bool("output_raw")?.unwrap_or(false);
        Ok(Self {
            filter: Arc::new(compile(&query).map_err(CoreError::config)?),
            in_raw,
            out_raw,
        })
    }

    /// Bento's `Process`: `Ok(None)` drops the message. Runs on the [`THREAD_STACK`] thread only.
    fn process(&self, part: &mut Part) -> Result<Option<()>, String> {
        let input = if self.in_raw {
            Val::from(String::from_utf8_lossy(&part.as_bytes()).into_owned())
        } else {
            to_val(&part.as_structured()?)?
        };
        let mut meta = BTreeMap::new();
        for (k, v) in part.meta_all() {
            meta.insert(k.clone(), v.clone());
        }
        let meta = to_val(&Value::Object(meta.into()))?;
        let emitted = run(&self.filter, input, meta)?;
        if self.out_raw {
            let mut raw = Vec::new();
            for (i, v) in emitted.iter().enumerate() {
                match v {
                    Value::String(s) => raw.extend_from_slice(s.as_bytes()),
                    other => raw.extend_from_slice(
                        other
                            .to_go_json_with(true, None)
                            .ok_or_else(|| {
                                format!("failed marshal JQ result at index {i}: json: unsupported value")
                            })?
                            .as_bytes(),
                    ),
                }
                if raw.len() > MAX_OUTPUT_BYTES {
                    return Err(format!(
                        "jq output is over {MAX_OUTPUT_BYTES} bytes, the most one message may hold"
                    ));
                }
            }
            if raw.is_empty() && emitted.is_empty() {
                return Ok(None);
            }
            part.set_bytes(raw);
            return Ok(Some(()));
        }
        let mut emitted = emitted;
        match emitted.len() {
            0 => return Ok(None),
            1 => {
                part.set_structured(emitted.remove(0));
            }
            _ => {
                part.set_structured(Value::Array(emitted.into()));
            }
        }
        Ok(Some(()))
    }
}

#[async_trait]
impl Processor for JqProcessor {
    fn processor_type(&self) -> &'static str {
        "jq"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        let this = Self {
            filter: self.filter.clone(),
            in_raw: self.in_raw,
            out_raw: self.out_raw,
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        std::thread::Builder::new()
            .name("ekiben-jq".into())
            .stack_size(THREAD_STACK)
            .spawn(move || {
                let results: Vec<_> = batch
                    .into_iter()
                    .map(|mut part| {
                        // A panic inside jaq fails this message, not the batch.
                        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            this.process(&mut part)
                        }))
                        .unwrap_or_else(|_| Err("jq query failed inside the evaluator".into()));
                        (part, r)
                    })
                    .collect();
                // The receiver is gone only when the stream stopped; the results go with it.
                let _ = tx.send(results);
            })
            .map_err(|e| CoreError::other(format!("jq: cannot start its thread: {e}")))?;
        let results = rx
            .await
            .map_err(|_| CoreError::other("jq: its thread ended without a result"))?;
        let mut out = Batch::new();
        for (mut part, r) in results {
            match r {
                Ok(Some(())) => out.push(part),
                Ok(None) => {}
                Err(e) => {
                    ctx.on_error(&mut part, e);
                    out.push(part);
                }
            }
        }
        if out.is_empty() {
            return Ok(Vec::new());
        }
        Ok(vec![out])
    }
}

/// gojq's `Parse` + `Compile` with `$metadata` as the one variable.
pub fn compile(query: &str) -> Result<JqFilter, String> {
    if query.len() > MAX_QUERY_BYTES {
        return Err(format!(
            "error parsing jq query: the query is {} bytes, more than {MAX_QUERY_BYTES}",
            query.len()
        ));
    }
    let extra = jaq_core::load::parse(GOJQ_DEFS, |p| p.defs())
        .ok_or_else(|| "error compiling jq query: built-in definitions".to_string())?;
    let defs = jaq_core::defs()
        .chain(jaq_std::defs())
        .chain(jaq_json::defs())
        .chain(extra);
    // jaq's `env` reads the process environment; gojq without an environ loader has none.
    let funs = jaq_core::funs()
        .chain(jaq_std::funs())
        .chain(jaq_json::funs())
        .filter(|(name, _, _)| *name != "env");
    let loader = Loader::new(defs);
    let arena = Arena::default();
    let modules = loader
        .load(
            &arena,
            File {
                code: query,
                path: (),
            },
        )
        .map_err(|errs| {
            let msg = errs
                .iter()
                .map(|(_, e)| format!("{e:?}"))
                .collect::<Vec<_>>()
                .join("; ");
            format!("error parsing jq query: {msg}")
        })?;
    Compiler::default()
        .with_funs(funs)
        .with_global_vars(["$metadata", "$ENV"])
        .compile(modules)
        .map_err(|errs| {
            let msg = errs
                .iter()
                .flat_map(|(_, es)| es.iter().map(|(name, undef)| format!("{undef:?} {name}")))
                .collect::<Vec<_>>()
                .join("; ");
            format!("error compiling jq query: {msg}")
        })
}

/// gojq's `Run` iterator, collected; the first error stops it, and so does the budget.
fn run(filter: &JqFilter, input: Val, meta: Val) -> Result<Vec<Value>, String> {
    let limits = jaq_core::budget::Limits {
        steps: MAX_STEPS,
        time: MAX_TIME,
        stack: MAX_EVAL_STACK,
        bytes: MAX_BUILT_BYTES,
    };
    let (out, exhausted) = jaq_core::budget::limit(limits, || run_query(filter, input, meta));
    match exhausted {
        Some(e) => Err(format!(
            "jq query stopped: {e} ({MAX_STEPS} steps, {} s, {} MiB of stack, {} MiB built per message)",
            MAX_TIME.as_secs(),
            MAX_EVAL_STACK >> 20,
            MAX_BUILT_BYTES >> 20
        )),
        None => out,
    }
}

fn run_query(filter: &JqFilter, input: Val, meta: Val) -> Result<Vec<Value>, String> {
    let env = Val::obj(Default::default());
    let ctx = Ctx::<JustLut<Val>>::new(&filter.lut, Vars::new([meta, env]));
    let mut out = Vec::new();
    for v in filter.id.run((ctx, input)) {
        match v {
            Ok(v) => out.push(from_val(&v, 0)?),
            Err(e) => {
                let e = match e.get_err() {
                    Ok(e) => capped_text(&e),
                    Err(_) => "jq query halted".to_string(),
                };
                return Err(e);
            }
        }
        if out.len() > MAX_OUTPUTS {
            return Err(format!(
                "jq query emitted more than {MAX_OUTPUTS} values for one message"
            ));
        }
    }
    Ok(out)
}

/// `e` as text, cut at [`MAX_ERROR_BYTES`]: `error(v)` may carry any value, shared parts of it
/// printed again each time they occur.
fn capped_text(e: &impl std::fmt::Display) -> String {
    struct Capped(String);
    impl std::fmt::Write for Capped {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            let room = MAX_ERROR_BYTES - self.0.len();
            if s.len() > room {
                let mut end = room;
                while !s.is_char_boundary(end) {
                    end -= 1;
                }
                self.0.push_str(&s[..end]);
                return Err(std::fmt::Error);
            }
            self.0.push_str(s);
            Ok(())
        }
    }
    let mut out = Capped(String::new());
    if std::fmt::write(&mut out, format_args!("{e}")).is_err() {
        out.0.push_str("...");
    }
    out.0
}

/// A value as gojq receives it from Bento (`gojq.normalizeNumbers` on `json.Number`).
pub fn to_val(v: &Value) -> Result<Val, String> {
    Ok(match v {
        Value::Null | Value::Delete | Value::Nothing => Val::Null,
        Value::Bool(b) => Val::from(*b),
        Value::Int(i) => int_val(*i as i128),
        Value::Uint(u) => int_val(*u as i128),
        Value::Float(f) => Val::from(*f),
        Value::Number(s) => number_val(s),
        Value::String(s) => Val::from(s.clone()),
        Value::Bytes(_) => return Err("invalid type: []uint8".to_string()),
        Value::Timestamp(t) => return Err(format!("invalid type: time.Time ({})", t.to_rfc3339())),
        Value::Array(a) => a.iter().map(to_val).collect::<Result<Val, String>>()?,
        Value::Object(m) => {
            let mut o = jaq_json::Map::default();
            for (k, v) in m {
                o.insert(Val::from(k.clone()), to_val(v)?);
            }
            Val::obj(o)
        }
    })
}

fn int_val(i: i128) -> Val {
    match isize::try_from(i) {
        Ok(i) => Val::from(i),
        Err(_) => Val::Num(Num::from_str_radix(&i.to_string(), 10).unwrap_or(Num::Float(i as f64))),
    }
}

/// gojq's `normalizeNumber`: an int64 stays an integer, a longer integer literal a big integer,
/// anything else a float64.
fn number_val(s: &str) -> Val {
    if let Ok(i) = s.parse::<i64>() {
        return int_val(i as i128);
    }
    if s.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
        if let Some(n) = Num::from_str_radix(s, 10) {
            return Val::Num(n);
        }
    }
    Val::from(s.parse::<f64>().unwrap_or(f64::NAN))
}

/// A gojq result as Bento holds it: integers as `int`, big integers verbatim, floats as `float64`.
fn from_val(v: &Val, depth: usize) -> Result<Value, String> {
    if depth > MAX_DEPTH {
        return Err(format!(
            "jq result is nested deeper than {MAX_DEPTH} levels"
        ));
    }
    Ok(match v {
        Val::Null => Value::Null,
        Val::Bool(b) => Value::Bool(*b),
        Val::Num(n) => match n {
            Num::Int(i) => Value::Int(*i as i64),
            Num::BigInt(b) => Value::Number(b.to_string()),
            Num::Float(f) => Value::Float(*f),
            Num::Dec(d) => Value::Float(d.parse::<f64>().unwrap_or(f64::NAN)),
        },
        Val::TStr(b) | Val::BStr(b) => Value::String(String::from_utf8_lossy(b).into_owned()),
        Val::Arr(a) => Value::Array(
            a.iter()
                .map(|v| from_val(v, depth + 1))
                .collect::<Result<Vec<_>, _>>()?
                .into(),
        ),
        Val::Obj(o) => {
            let mut m = BTreeMap::new();
            for (k, v) in o.iter() {
                let key = match k {
                    Val::TStr(b) | Val::BStr(b) => String::from_utf8_lossy(b).into_owned(),
                    other => return Err(format!("object key {other} is not a string")),
                };
                m.insert(key, from_val(v, depth + 1)?);
            }
            Value::Object(m.into())
        }
    })
}
