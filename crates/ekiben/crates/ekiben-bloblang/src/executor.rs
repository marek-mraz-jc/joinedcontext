use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use crate::ast::{
    AssignTargetPath, Assignment, BinaryOp, Expr, MapDef, MatchPattern, Statement, TargetPath,
    TargetsContext,
};
use crate::env::Environment;
use crate::error::BloblangError;
use crate::functions::eval_function;
use crate::methods::eval_method;
use crate::value::Value;

/// Information about a processor error attached to a message part.
#[derive(Debug, Clone, Default)]
pub struct DetailedErrorInfo {
    pub message: String,
    pub label: String,
    pub component_type: String,
    pub path: String,
}

/// Trait implemented by `ekiben_core::PartsView<'a>` (a local wrapper type, respecting Rust's orphan rule)
/// so the Bloblang interpreter can inspect any part in the current batch without depending on `ekiben-core`.
pub trait MessageBatch {
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn part_bytes(&self, index: usize) -> &[u8];
    fn part_structured(&self, index: usize) -> Result<Value, String>;
    fn part_meta_get(&self, index: usize, key: &str) -> Option<Value>;
    fn part_meta_all(&self, index: usize) -> BTreeMap<String, Value>;
    fn part_error(&self, index: usize) -> Option<DetailedErrorInfo>;
}

/// Trait for mutating metadata on the target message part being mapped.
pub trait PartMetaMut {
    fn meta_get(&self, key: &str) -> Option<Value>;
    fn meta_set(&mut self, key: String, val: Value);
    fn meta_delete(&mut self, key: &str);
    fn meta_clear(&mut self);
    fn meta_all(&self) -> BTreeMap<String, Value>;
}

/// Minimal standalone `MessageBatch` for single-value or unit-test evaluations.
pub struct EmptyBatch;

impl MessageBatch for EmptyBatch {
    fn len(&self) -> usize {
        0
    }
    fn part_bytes(&self, _index: usize) -> &[u8] {
        &[]
    }
    fn part_structured(&self, _index: usize) -> Result<Value, String> {
        Err("message is empty".to_string())
    }
    fn part_meta_get(&self, _index: usize, _key: &str) -> Option<Value> {
        None
    }
    fn part_meta_all(&self, _index: usize) -> BTreeMap<String, Value> {
        BTreeMap::new()
    }
    fn part_error(&self, _index: usize) -> Option<DetailedErrorInfo> {
        None
    }
}

/// Simple standalone part view for `ekiben blobl` CLI and unit tests.
#[derive(Clone, Debug, Default)]
pub struct SimplePartView {
    pub raw: Vec<u8>,
    pub structured: Option<Value>,
    pub meta: BTreeMap<String, Value>,
    pub error: Option<DetailedErrorInfo>,
}

impl SimplePartView {
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            raw: bytes.into(),
            structured: None,
            meta: BTreeMap::new(),
            error: None,
        }
    }

    pub fn from_value(v: Value) -> Self {
        Self {
            raw: v.to_bytes_coerced(),
            structured: Some(v),
            meta: BTreeMap::new(),
            error: None,
        }
    }
}

impl PartMetaMut for BTreeMap<String, Value> {
    fn meta_get(&self, key: &str) -> Option<Value> {
        self.get(key).cloned()
    }
    fn meta_set(&mut self, key: String, val: Value) {
        self.insert(key, val);
    }
    fn meta_delete(&mut self, key: &str) {
        self.remove(key);
    }
    fn meta_clear(&mut self) {
        self.clear();
    }
    fn meta_all(&self) -> BTreeMap<String, Value> {
        self.clone()
    }
}

impl<const N: usize> MessageBatch for [SimplePartView; N] {
    fn len(&self) -> usize {
        <[SimplePartView]>::len(self)
    }
    fn part_bytes(&self, index: usize) -> &[u8] {
        self.get(index).map(|p| p.raw.as_slice()).unwrap_or(&[])
    }
    fn part_structured(&self, index: usize) -> Result<Value, String> {
        let Some(p) = self.get(index) else {
            return Err("message is empty".to_string());
        };
        if let Some(s) = &p.structured {
            return Ok(s.clone());
        }
        if p.raw.is_empty() {
            return Err("message is empty".to_string());
        }
        if let Some(v) = crate::go_json::decode_with_numbers(&p.raw) {
            return Ok(v);
        }
        let jv: serde_json::Value =
            serde_json::from_slice(&p.raw).map_err(|e| format!("parse as json: {e}"))?;
        Ok(Value::from_json_value(jv))
    }
    fn part_meta_get(&self, index: usize, key: &str) -> Option<Value> {
        self.get(index).and_then(|p| p.meta.get(key).cloned())
    }
    fn part_meta_all(&self, index: usize) -> BTreeMap<String, Value> {
        self.get(index).map(|p| p.meta.clone()).unwrap_or_default()
    }
    fn part_error(&self, index: usize) -> Option<DetailedErrorInfo> {
        self.get(index).and_then(|p| p.error.clone())
    }
}

/// A compiled Bloblang mapping ready for concurrent execution across worker threads.
#[derive(Clone)]
pub struct Executor {
    annotation: String,
    maps: Arc<HashMap<String, Arc<MapDef>>>,
    statements: Arc<Vec<Statement>>,
    env: Environment,
    named_counters: Arc<Mutex<HashMap<String, i64>>>,
    stateful_counters: Arc<Mutex<HashMap<usize, i64>>>,
    rng_counter: Arc<AtomicU64>,
}

impl Executor {
    pub fn new(
        annotation: String,
        maps: Arc<HashMap<String, Arc<MapDef>>>,
        statements: Vec<Statement>,
        env: Environment,
    ) -> Self {
        Self {
            annotation,
            maps,
            statements: Arc::new(statements),
            env,
            named_counters: Arc::new(Mutex::new(HashMap::new())),
            stateful_counters: Arc::new(Mutex::new(HashMap::new())),
            rng_counter: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn annotation(&self) -> &str {
        &self.annotation
    }

    /// Collects all `TargetPath`s referenced by queries in this mapping (cycle-safe over recursive maps).
    pub fn query_targets(&self) -> Vec<TargetPath> {
        let mut ctx = TargetsContext {
            maps: self.maps.clone(),
            ..Default::default()
        };
        let mut out = Vec::new();
        for stmt in self.statements.iter() {
            let (nc, paths) = stmt.query_targets(ctx);
            ctx = nc;
            out.extend(paths);
        }
        out
    }

    /// Collects all `AssignTargetPath`s written by statements in this mapping.
    pub fn assignment_targets(&self) -> Vec<AssignTargetPath> {
        let mut out = Vec::new();
        for stmt in self.statements.iter() {
            out.extend(stmt.assignment_targets());
        }
        out
    }

    /// Executes the mapping on a standalone `Value` (for unit tests, previews, and `.bloblang()`).
    pub fn exec_on_value(&self, input: Value) -> Result<Value, BloblangError> {
        let parts = [SimplePartView::from_value(input)];
        let mut meta = BTreeMap::new();
        let res = self.map_part(0, &parts, &mut meta, None)?;
        Ok(res.unwrap_or(Value::Delete))
    }

    /// Executes the mapping on raw JSON/text bytes and returns the mapped `Value` (or `None` if `deleted()`).
    pub fn execute_bytes(&self, raw: &[u8]) -> Result<Option<Value>, BloblangError> {
        let parts = [SimplePartView::from_bytes(raw.to_vec())];
        let mut meta = BTreeMap::new();
        self.map_part(0, &parts, &mut meta, None)
    }

    /// As `execute_bytes`, with metadata read in place (`mutate_part`), as Bento's `blobl` CLI runs a line.
    pub fn execute_bytes_in_place(&self, raw: &[u8]) -> Result<Option<Value>, BloblangError> {
        let parts = [SimplePartView::from_bytes(raw.to_vec())];
        let mut meta = BTreeMap::new();
        self.mutate_part(0, &parts, &mut meta, None)
    }

    /// Executes the mapping as a boolean check query on part `index` of `batch` (`Executor.QueryPart`).
    pub fn query_part(
        &self,
        index: usize,
        batch: &dyn MessageBatch,
    ) -> Result<bool, BloblangError> {
        // `@` in a check reads the message's own metadata, as in Bento's `QueryPart`.
        let mut meta = batch.part_meta_all(index);
        let res = self.map_part(index, batch, &mut meta, None)?;
        match res {
            None => Err(BloblangError::runtime(
                "query mapping resulted in deleted message, expected a boolean value",
            )),
            Some(Value::Bool(b)) => Ok(b),
            Some(other) => Err(BloblangError::runtime(format!(
                "mapping: expected bool value, got {} ({})",
                other.type_name(),
                other.to_display_short()
            ))),
        }
    }

    /// Executes the mapping on `index` of `batch`.
    /// - If `append_onto` is `None` (`mapping` processor): starts `root` as `Value::Nothing`;
    ///   if `root` is still `Value::Nothing` at the end, returns the original part unchanged!
    /// - If `append_onto` is `Some(initial)` (`mutation` processor / `branch.result_map`):
    ///   starts `root` as `initial` and mutates it in place.
    /// - If the final `root` is `Value::Delete`, returns `Ok(None)` so the caller drops the part.
    pub fn map_part(
        &self,
        index: usize,
        batch: &dyn MessageBatch,
        new_meta: &mut dyn PartMetaMut,
        append_onto: Option<Value>,
    ) -> Result<Option<Value>, BloblangError> {
        self.run_part(index, batch, new_meta, append_onto, false)
    }

    /// Executes the mapping in place on part `index`, as Bento's `mutation` processor and `blobl` CLI do:
    /// `metadata()` and `meta()` read the metadata as the mapping has written it so far.
    pub fn mutate_part(
        &self,
        index: usize,
        batch: &dyn MessageBatch,
        new_meta: &mut dyn PartMetaMut,
        append_onto: Option<Value>,
    ) -> Result<Option<Value>, BloblangError> {
        self.run_part(index, batch, new_meta, append_onto, true)
    }

    fn run_part(
        &self,
        index: usize,
        batch: &dyn MessageBatch,
        new_meta: &mut dyn PartMetaMut,
        append_onto: Option<Value>,
        meta_in_place: bool,
    ) -> Result<Option<Value>, BloblangError> {
        let mut new_value = append_onto.unwrap_or(Value::Nothing);
        let mut vars = HashMap::new();

        let mut ctx = EvalContext {
            env: &self.env,
            maps: &self.maps,
            vars: &mut vars,
            index,
            batch,
            new_meta: Some(new_meta),
            new_value: &mut new_value,
            cached_root_value: None,
            value_stack: Vec::new(),
            named_values: Vec::new(),
            stack_count: 0,
            max_stacks: self.env.max_map_recursion,
            named_counters: &self.named_counters,
            stateful_counters: &self.stateful_counters,
            rng_counter: &self.rng_counter,
            meta_in_place,
        };

        for stmt in self.statements.iter() {
            exec_statement(stmt, &mut ctx)?;
        }

        match new_value {
            Value::Delete => Ok(None),
            Value::Nothing => Ok(Some(Value::Nothing)),
            other => Ok(Some(other)),
        }
    }
}

pub(crate) struct EvalContext<'a, 'm> {
    pub env: &'a Environment,
    pub maps: &'a Arc<HashMap<String, Arc<MapDef>>>,
    pub vars: &'a mut HashMap<String, Value>,
    pub index: usize,
    pub batch: &'a dyn MessageBatch,
    pub new_meta: Option<&'m mut dyn PartMetaMut>,
    pub new_value: &'a mut Value,
    pub cached_root_value: Option<Result<Value, String>>,
    pub value_stack: Vec<Value>,
    pub named_values: Vec<(String, Value)>,
    pub stack_count: usize,
    pub max_stacks: usize,
    pub named_counters: &'a Arc<Mutex<HashMap<String, i64>>>,
    pub stateful_counters: &'a Arc<Mutex<HashMap<usize, i64>>>,
    pub rng_counter: &'a Arc<AtomicU64>,
    /// `metadata()`/`meta()` read `new_meta` instead of the input part (`mutate_part`).
    pub meta_in_place: bool,
}

impl<'a, 'm> EvalContext<'a, 'm> {
    pub fn current_value(&mut self, field_hint: &str) -> Result<Value, String> {
        let path = crate::value::dot_path_to_slice(field_hint);
        self.current_value_ref(&path).cloned()
    }

    /// The context `this` reads, borrowed: a path read clones only what it reaches, not the
    /// whole document (a fold over a large tally read it once per field reference).
    pub fn current_value_ref(&mut self, path: &[String]) -> Result<&Value, String> {
        if self.value_stack.is_empty() && self.cached_root_value.is_none() {
            self.cached_root_value = Some(self.batch.part_structured(self.index));
        }
        if let Some(top) = self.value_stack.last() {
            return Ok(top);
        }
        match self
            .cached_root_value
            .as_ref()
            .ok_or_else(|| "message is empty".to_string())?
        {
            Ok(v) => Ok(v),
            Err(parse_err) => {
                let field_hint = crate::value::slice_to_dot_path(path);
                if field_hint.is_empty() {
                    Err(format!(
                        "unable to reference message as structured (with 'this'): {parse_err}"
                    ))
                } else {
                    Err(format!(
                        "unable to reference message as structured (with 'this.{field_hint}'): {parse_err}"
                    ))
                }
            }
        }
    }

    pub fn eval_with_value(&mut self, val: Value, expr: &Expr) -> Result<Value, String> {
        self.value_stack.push(val);
        let res = self.eval_expr(expr);
        self.value_stack.pop();
        res
    }

    pub fn exec_named_map(&mut self, map_name: &str, input_val: Value) -> Result<Value, String> {
        self.stack_count += 1;
        if self.stack_count > self.max_stacks {
            let err = BloblangError::MaxStacksExceeded {
                annotation: format!("map {map_name}"),
                max_stacks: self.max_stacks,
            };
            self.stack_count -= 1;
            return Err(err.to_string());
        }

        let Some(map_def) = self.maps.get(map_name).cloned() else {
            self.stack_count -= 1;
            return Err(format!("map {map_name} was not found"));
        };

        let res = stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            let mut map_root = Value::Nothing;
            let mut map_vars = HashMap::new();
            let mut sub_ctx = EvalContext {
                env: self.env,
                maps: self.maps,
                vars: &mut map_vars,
                index: self.index,
                batch: self.batch,
                new_meta: None,
                new_value: &mut map_root,
                cached_root_value: Some(Ok(input_val.clone())),
                value_stack: vec![input_val],
                named_values: Vec::new(),
                stack_count: self.stack_count,
                max_stacks: self.max_stacks,
                named_counters: self.named_counters,
                stateful_counters: self.stateful_counters,
                rng_counter: self.rng_counter,
                meta_in_place: false,
            };
            for stmt in &map_def.statements {
                if let Err(e) = exec_statement(stmt, &mut sub_ctx) {
                    return Err(e.to_string());
                }
            }
            Ok(map_root)
        });

        self.stack_count -= 1;
        res
    }

    pub fn eval_expr(&mut self, expr: &Expr) -> Result<Value, String> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || match expr {
            Expr::Literal(v) => Ok(v.clone()),
            Expr::ArrayLiteral(items) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    let v = self.eval_expr(item)?;
                    if !matches!(v, Value::Delete | Value::Nothing) {
                        out.push(v);
                    }
                }
                Ok(Value::Array(out.into()))
            }
            Expr::ObjectLiteral(kvs) => {
                let mut out = BTreeMap::new();
                for (k_expr, v_expr) in kvs {
                    let k_val = self
                        .eval_expr(k_expr)
                        .map_err(|e| format!("failed to resolve key: {e}"))?;
                    let key = match k_val {
                        Value::String(s) => s,
                        Value::Bytes(b) => String::from_utf8_lossy(&b).into_owned(),
                        other => {
                            return Err(format!(
                                "mapping returned invalid key type: {}",
                                other.type_name()
                            ));
                        }
                    };
                    let val = self
                        .eval_expr(v_expr)
                        .map_err(|e| format!("failed to resolve '{key}' value: {e}"))?;
                    if !matches!(val, Value::Delete | Value::Nothing) {
                        out.insert(key, val);
                    }
                }
                Ok(Value::Object(out.into()))
            }
            Expr::FieldRef {
                named_context,
                from_root,
                path,
            } => {
                let target: &Value = if *from_root {
                    self.new_value
                } else if let Some(nc) = named_context {
                    self.named_values
                        .iter()
                        .rev()
                        .find(|(k, _)| k == nc)
                        .map(|(_, v)| v)
                        .ok_or_else(|| format!("named context {nc} was not found"))?
                } else {
                    self.current_value_ref(path)?
                };
                Ok(target.get_path(path))
            }
            Expr::VarRef(name) => self
                .vars
                .get(name)
                .cloned()
                .ok_or_else(|| format!("variable '{name}' undefined")),
            Expr::MetaRef(Some(key)) => {
                if let Some(meta) = self.new_meta.as_deref() {
                    Ok(meta.meta_get(key).unwrap_or(Value::Null))
                } else {
                    Ok(self
                        .batch
                        .part_meta_get(self.index, key)
                        .unwrap_or(Value::Null))
                }
            }
            Expr::MetaRef(None) => {
                if let Some(meta) = self.new_meta.as_deref() {
                    Ok(Value::Object(meta.meta_all().into()))
                } else {
                    Ok(Value::Object(self.batch.part_meta_all(self.index).into()))
                }
            }
            Expr::FunctionCall {
                name,
                args,
                state_id,
            } => eval_function(name, args, *state_id, self),
            Expr::MethodCall { target, name, args } => eval_method(target, name, args, self),
            Expr::MapExpr { target, map_fn } => {
                let t_val = self.eval_expr(target)?;
                self.eval_with_value(t_val, map_fn)
            }
            Expr::IndexExpr { target, index } => {
                let t_val = self.eval_expr(target)?;
                let idx = self
                    .eval_expr(index)?
                    .get_int()
                    .map_err(|e| format!("index: {e}"))?;
                match t_val {
                    Value::String(s) => {
                        let chars: Vec<char> = s.chars().collect();
                        let len = chars.len() as i64;
                        let norm = if idx < 0 { len + idx } else { idx };
                        if norm < 0 || norm >= len {
                            return Err(format!("index {norm} out of bounds for length {len}"));
                        }
                        Ok(Value::String(chars[norm as usize].to_string()))
                    }
                    Value::Bytes(b) => {
                        let len = b.len() as i64;
                        let norm = if idx < 0 { len + idx } else { idx };
                        if norm < 0 || norm >= len {
                            return Err(format!("index {norm} out of bounds for length {len}"));
                        }
                        Ok(Value::Int(b[norm as usize] as i64))
                    }
                    Value::Array(a) => {
                        let len = a.len() as i64;
                        let norm = if idx < 0 { len + idx } else { idx };
                        if norm < 0 || norm >= len {
                            return Err(format!("index {norm} out of bounds for length {len}"));
                        }
                        Ok(a[norm as usize].clone())
                    }
                    other => Err(format!(
                        "expected array or string value, got {}",
                        other.type_name()
                    )),
                }
            }
            Expr::SliceExpr {
                target,
                start,
                end,
                step,
            } => {
                let t_val = self.eval_expr(target)?;
                let s_bound = match start {
                    Some(e) => Some(
                        self.eval_expr(e)?
                            .get_int()
                            .map_err(|err| format!("slice start bound: {err}"))?,
                    ),
                    None => None,
                };
                let e_bound = match end {
                    Some(e) => Some(
                        self.eval_expr(e)?
                            .get_int()
                            .map_err(|err| format!("slice end bound: {err}"))?,
                    ),
                    None => None,
                };
                let st_bound = match step {
                    Some(e) => {
                        let st = self
                            .eval_expr(e)?
                            .get_int()
                            .map_err(|err| format!("slice step: {err}"))?;
                        if st == 0 {
                            return Err("slice step cannot be zero".to_string());
                        }
                        Some(st)
                    }
                    None => None,
                };
                if let (Some(sb), Some(eb), None) = (s_bound, e_bound, st_bound) {
                    if sb >= 0 && eb >= 0 && sb > eb {
                        return Err(format!(
                            "lower slice bound ({sb}) must be lower than upper ({eb})"
                        ));
                    }
                }
                match t_val {
                    Value::String(s) => {
                        let chars: Vec<char> = s.chars().collect();
                        let sliced = slice_items(&chars, s_bound, e_bound, st_bound);
                        Ok(Value::String(sliced.into_iter().collect()))
                    }
                    Value::Bytes(b) => {
                        let sliced = slice_items(&b, s_bound, e_bound, st_bound);
                        Ok(Value::Bytes(sliced))
                    }
                    Value::Array(a) => {
                        let sliced = slice_items(&a, s_bound, e_bound, st_bound);
                        Ok(Value::Array(sliced.into()))
                    }
                    other => Err(format!(
                        "expected array or string value, got {}",
                        other.type_name()
                    )),
                }
            }
            Expr::Not(inner) => {
                let v = self.eval_expr(inner)?;
                match v {
                    Value::Bool(b) => Ok(Value::Bool(!b)),
                    other => Err(crate::value::type_error_from(
                        &inner.annotation(),
                        &other,
                        &["bool"],
                    )),
                }
            }
            Expr::Binary { op, lhs, rhs } => eval_binary_op(*op, lhs, rhs, self),
            Expr::If {
                cond,
                then_branch,
                else_ifs,
                else_branch,
            } => {
                let c_val = self
                    .eval_expr(cond)
                    .map_err(|e| format!("failed to check if condition: {e}"))?;
                let c_bool = match c_val {
                    Value::Bool(b) => b,
                    Value::Null => false,
                    other => {
                        return Err(format!(
                            "{} resolved to a non-boolean value {} ({})",
                            cond.annotation(),
                            other.to_display_short(),
                            other.type_name()
                        ));
                    }
                };
                if c_bool {
                    return self.eval_expr(then_branch);
                }
                for (i, (econd, ebody)) in else_ifs.iter().enumerate() {
                    let ev = self
                        .eval_expr(econd)
                        .map_err(|e| format!("failed to check if condition {}: {e}", i + 1))?;
                    let eb = match ev {
                        Value::Bool(b) => b,
                        Value::Null => false,
                        other => {
                            return Err(format!(
                                "{} resolved to a non-boolean value {} ({})",
                                econd.annotation(),
                                other.to_display_short(),
                                other.type_name()
                            ));
                        }
                    };
                    if eb {
                        return self.eval_expr(ebody);
                    }
                }
                if let Some(el) = else_branch {
                    self.eval_expr(el)
                } else {
                    Ok(Value::Nothing)
                }
            }
            Expr::Match {
                context_expr,
                cases,
            } => {
                let ctx_val = match context_expr {
                    Some(ce) => self.eval_expr(ce)?,
                    None => self.current_value("").unwrap_or(Value::Null),
                };
                self.value_stack.push(ctx_val.clone());
                let mut result = Ok(Value::Nothing);
                for (i, c) in cases.iter().enumerate() {
                    let matched = match &c.pattern {
                        MatchPattern::Wildcard => Ok(true),
                        MatchPattern::ValueLit(lit) => Ok(ctx_val.compare_eq(lit)),
                        MatchPattern::Condition(cond_expr) => self
                            .eval_expr(cond_expr)
                            .map_err(|e| format!("failed to check match case {i}: {e}"))
                            .map(|v| matches!(v, Value::Bool(true))),
                    };
                    match matched {
                        Ok(true) => {
                            result = self.eval_expr(&c.query);
                            break;
                        }
                        Ok(false) => {}
                        Err(e) => {
                            result = Err(e);
                            break;
                        }
                    }
                }
                self.value_stack.pop();
                result
            }
            Expr::Lambda { name, body } => {
                let captured = self.value_stack.pop().ok_or_else(|| {
                    format!("failed to capture context {name}: context was undefined")
                })?;
                if name != "_" {
                    self.named_values.push((name.clone(), captured.clone()));
                }
                let res = self.eval_expr(body);
                if name != "_" {
                    self.named_values.pop();
                }
                self.value_stack.push(captured);
                res
            }
        })
    }
}

fn slice_items<T: Clone>(
    data: &[T],
    start: Option<i64>,
    end: Option<i64>,
    step: Option<i64>,
) -> Vec<T> {
    let len = data.len() as i64;
    let step_val = step.unwrap_or(1);
    let start_idx = match start {
        None => {
            if step_val > 0 {
                0
            } else {
                len - 1
            }
        }
        Some(s) => {
            let mut idx = if s < 0 { (len + s).max(0) } else { s };
            if step_val > 0 {
                idx = idx.min(len);
            } else {
                idx = idx.min(len - 1);
            }
            idx
        }
    };
    let end_idx = match end {
        None => {
            if step_val > 0 {
                len
            } else {
                -1
            }
        }
        Some(e) => {
            let mut idx = if e < 0 { (len + e).max(-1) } else { e };
            if step_val > 0 {
                idx = idx.min(len);
            }
            idx
        }
    };

    if step_val == 1 {
        if start_idx >= end_idx || start_idx < 0 {
            return Vec::new();
        }
        return data[start_idx as usize..end_idx as usize].to_vec();
    }

    let mut out = Vec::new();
    if step_val > 0 {
        let mut i = start_idx;
        while i < end_idx {
            if i >= 0 && i < len {
                out.push(data[i as usize].clone());
            }
            i += step_val;
        }
    } else {
        let mut i = start_idx;
        while i > end_idx {
            if i >= 0 && i < len {
                out.push(data[i as usize].clone());
            }
            i += step_val;
        }
    }
    out
}

fn eval_binary_op(
    op: BinaryOp,
    lhs: &Expr,
    rhs: &Expr,
    ctx: &mut EvalContext<'_, '_>,
) -> Result<Value, String> {
    match op {
        BinaryOp::Pipe => match ctx.eval_expr(lhs) {
            Ok(v) if !v.is_null() => Ok(v),
            Err(e)
                if e.starts_with("entering ") && e.contains("exceeded maximum allowed stacks") =>
            {
                Err(e)
            }
            _ => ctx.eval_expr(rhs),
        },
        BinaryOp::And => {
            let lv = ctx.eval_expr(lhs)?;
            let lb = lv.get_bool_go()?;
            if !lb {
                return Ok(Value::Bool(false));
            }
            let rv = ctx.eval_expr(rhs)?;
            let rb = rv.get_bool_go()?;
            Ok(Value::Bool(rb))
        }
        BinaryOp::Or => {
            let lv = ctx.eval_expr(lhs)?;
            let lb = lv.get_bool_go()?;
            if lb {
                return Ok(Value::Bool(true));
            }
            let rv = ctx.eval_expr(rhs)?;
            let rb = rv.get_bool_go()?;
            Ok(Value::Bool(rb))
        }
        _ => {
            // Arithmetic and comparison read a json.Number as ISanitize does.
            let lv = ctx.eval_expr(lhs)?.into_sanitized();
            let rv = ctx.eval_expr(rhs)?.into_sanitized();
            let mismatch = || {
                format!(
                    "cannot {} types {} (from {}) and {} (from {})",
                    op.name(),
                    lv.type_name(),
                    lhs.annotation(),
                    rv.type_name(),
                    rhs.annotation()
                )
            };
            match op {
                BinaryOp::Add => match (&lv, &rv) {
                    (Value::String(_) | Value::Bytes(_), Value::String(_) | Value::Bytes(_)) => {
                        // Go joins the raw bytes into a string. ponytail: a join that is not
                        // UTF-8 (a zip built in a mapping) stays bytes, which a Rust String cannot
                        // hold; type() then reads "bytes" where Bento reads "string".
                        let mut joined = byte_text(&lv).to_vec();
                        joined.extend_from_slice(byte_text(&rv));
                        Ok(match String::from_utf8(joined) {
                            Ok(s) => Value::String(s),
                            Err(e) => Value::Bytes(e.into_bytes()),
                        })
                    }
                    _ => eval_numeric_degradation(
                        &lv,
                        &rv,
                        op,
                        mismatch,
                        |a, b| a.wrapping_add(b),
                        |a, b| a + b,
                    ),
                },
                BinaryOp::Sub => eval_numeric_degradation(
                    &lv,
                    &rv,
                    op,
                    mismatch,
                    |a, b| a.wrapping_sub(b),
                    |a, b| a - b,
                ),
                BinaryOp::Mul => eval_numeric_degradation(
                    &lv,
                    &rv,
                    op,
                    mismatch,
                    |a, b| a.wrapping_mul(b),
                    |a, b| a * b,
                ),
                BinaryOp::Div => {
                    let a = lv.as_f64().ok_or_else(mismatch)?;
                    let b = rv.as_f64().ok_or_else(mismatch)?;
                    if b == 0.0 {
                        return Err(format!("{}: attempted to divide by zero", rhs.annotation()));
                    }
                    Ok(Value::Float(a / b))
                }
                BinaryOp::Mod => {
                    // Bento's modulo is integer only: a float operand is truncated to int64.
                    let as_int = |v: &Value| match v {
                        Value::Float(f) if f.is_finite() => Some(*f as i64),
                        other => other.get_int().ok(),
                    };
                    let a = as_int(&lv).ok_or_else(mismatch)?;
                    let b = as_int(&rv).ok_or_else(mismatch)?;
                    if b == 0 {
                        return Err(format!("{}: attempted to divide by zero", rhs.annotation()));
                    }
                    Ok(Value::Int(a % b))
                }
                BinaryOp::Eq => Ok(Value::Bool(compare_eq_or_bytes(&lv, &rv))),
                BinaryOp::Neq => Ok(Value::Bool(!compare_eq_or_bytes(&lv, &rv))),
                BinaryOp::Gt | BinaryOp::Gte | BinaryOp::Lt | BinaryOp::Lte => match (&lv, &rv) {
                    (Value::String(_) | Value::Bytes(_), Value::String(_) | Value::Bytes(_)) => {
                        let s1 = lv.get_string().map_err(|_| mismatch())?;
                        let s2 = rv.get_string().map_err(|_| mismatch())?;
                        let b = match op {
                            BinaryOp::Gt => s1 > s2,
                            BinaryOp::Gte => s1 >= s2,
                            BinaryOp::Lt => s1 < s2,
                            BinaryOp::Lte => s1 <= s2,
                            _ => unreachable!(),
                        };
                        Ok(Value::Bool(b))
                    }
                    _ => {
                        let a = lv.as_f64().ok_or_else(mismatch)?;
                        let b = rv.as_f64().ok_or_else(mismatch)?;
                        let res = match op {
                            BinaryOp::Gt => a > b,
                            BinaryOp::Gte => a >= b,
                            BinaryOp::Lt => a < b,
                            BinaryOp::Lte => a <= b,
                            _ => unreachable!(),
                        };
                        Ok(Value::Bool(res))
                    }
                },
                _ => unreachable!(),
            }
        }
    }
}

/// Bento's `==` (`compareOp` in internal/bloblang/query/arithmetic.go): the left operand's type picks the
/// coercion, so `true == 7` holds (7 reads as a bool) while `1 == true` does not.
fn compare_eq_or_bytes(lv: &Value, rv: &Value) -> bool {
    match lv {
        Value::String(_) | Value::Bytes(_) | Value::Timestamp(_) | Value::Bool(_) => {
            icompare(lv, rv)
        }
        Value::Int(_) | Value::Uint(_) | Value::Float(_) => match rv {
            Value::Int(_) | Value::Uint(_) | Value::Float(_) => lv.compare_eq(rv),
            _ => false,
        },
        _ => icompare(lv, rv),
    }
}

/// Bento's `value.ICompare`: the left side restricted (integers as float64, bytes as a string), the right
/// side read as the left side's type.
fn icompare(l: &Value, r: &Value) -> bool {
    if matches!(l, Value::Number(_)) || matches!(r, Value::Number(_)) {
        // A json.Number inside an array or object compares as the number it reads as.
        return icompare(&l.sanitized(), &r.sanitized());
    }
    let as_string = |v: &Value| match v {
        Value::String(s) => Some(s.clone()),
        Value::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
        Value::Timestamp(ts) => Some(ts.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)),
        _ => None,
    };
    match l {
        Value::Null => matches!(r, Value::Null),
        Value::String(_) | Value::Bytes(_) | Value::Timestamp(_) => as_string(l) == as_string(r),
        Value::Int(_) | Value::Uint(_) | Value::Float(_) => match (l.as_f64(), r) {
            (Some(a), Value::Int(_) | Value::Uint(_) | Value::Float(_)) => Some(a) == r.as_f64(),
            _ => false,
        },
        Value::Bool(a) => match r {
            Value::Bool(b) => a == b,
            Value::Int(_) | Value::Uint(_) | Value::Float(_) => {
                r.as_f64().map(|f| f != 0.0) == Some(*a)
            }
            _ => false,
        },
        Value::Array(a) => match r {
            Value::Array(b) => a.len() == b.len() && a.iter().zip(b).all(|(x, y)| icompare(x, y)),
            _ => false,
        },
        Value::Object(a) => match r {
            Value::Object(b) => {
                a.len() == b.len()
                    && a.iter()
                        .all(|(k, v)| icompare(v, b.get(k).unwrap_or(&Value::Null)))
            }
            _ => false,
        },
        _ => false,
    }
}

fn eval_numeric_degradation(
    lv: &Value,
    rv: &Value,
    _op: BinaryOp,
    mismatch: impl FnOnce() -> String,
    int_op: impl FnOnce(i64, i64) -> i64,
    float_op: impl FnOnce(f64, f64) -> f64,
) -> Result<Value, String> {
    if matches!(lv, Value::Float(_)) || matches!(rv, Value::Float(_)) {
        let Some(a) = lv.as_f64() else {
            return Err(mismatch());
        };
        let Some(b) = rv.as_f64() else {
            return Err(mismatch());
        };
        return Ok(Value::Float(float_op(a, b)));
    }
    match (lv.get_int(), rv.get_int()) {
        (Ok(a), Ok(b)) => Ok(Value::Int(int_op(a, b))),
        _ => Err(mismatch()),
    }
}

fn exec_statement(stmt: &Statement, ctx: &mut EvalContext<'_, '_>) -> Result<(), BloblangError> {
    stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || match stmt {
        Statement::Single {
            line,
            assignment,
            query,
        } => {
            let val = ctx
                .eval_expr(query)
                .map_err(|e| BloblangError::Assignment {
                    line: *line,
                    message: e,
                })?;
            if matches!(val, Value::Nothing) {
                return Ok(());
            }
            apply_assignment(assignment, val, ctx).map_err(|e| BloblangError::Assignment {
                line: *line,
                message: e,
            })
        }
        Statement::If { line, branches } => {
            for (i, (cond_opt, stmts)) in branches.iter().enumerate() {
                if let Some(cond) = cond_opt {
                    let c_val = ctx.eval_expr(cond).map_err(|e| BloblangError::Assignment {
                        line: *line,
                        message: format!("failed to check if condition {}: {e}", i + 1),
                    })?;
                    let c_bool = match c_val {
                        Value::Bool(b) => b,
                        Value::Null => false,
                        other => {
                            return Err(BloblangError::Assignment {
                                line: *line,
                                message: format!(
                                    "{} resolved to a non-boolean value {} ({})",
                                    cond.annotation(),
                                    other.to_display_short(),
                                    other.type_name()
                                ),
                            });
                        }
                    };
                    if !c_bool {
                        continue;
                    }
                }
                // Bento wraps a failure once, at the top-level statement (`mapping.Executor`):
                // one inside an `if` block carries the line of the outermost statement.
                for s in stmts {
                    exec_statement(s, ctx).map_err(|e| match e {
                        BloblangError::Assignment { message, .. } => BloblangError::Assignment {
                            line: *line,
                            message,
                        },
                        other => other,
                    })?;
                }
                return Ok(());
            }
            Ok(())
        }
    })
}

fn apply_assignment(
    assignment: &Assignment,
    val: Value,
    ctx: &mut EvalContext<'_, '_>,
) -> Result<(), String> {
    match assignment {
        Assignment::Var(name) => {
            if matches!(val, Value::Delete) {
                ctx.vars.remove(name);
            } else {
                ctx.vars.insert(name.clone(), val);
            }
            Ok(())
        }
        Assignment::Meta(key_opt) => {
            let Some(meta) = ctx.new_meta.as_deref_mut() else {
                return Err("unable to assign metadata in the current context".to_string());
            };
            match key_opt {
                None => {
                    if matches!(val, Value::Delete) {
                        meta.meta_clear();
                    } else if let Value::Object(map) = val {
                        meta.meta_clear();
                        for (k, v) in map {
                            meta.meta_set(k, v);
                        }
                    } else {
                        return Err(format!(
                            "setting root meta object requires object value, received: {}",
                            val.type_name()
                        ));
                    }
                }
                Some(key) => {
                    if matches!(val, Value::Delete) {
                        meta.meta_delete(key);
                    } else {
                        meta.meta_set(key.clone(), val);
                    }
                }
            }
            Ok(())
        }
        Assignment::Json(path) => {
            if matches!(val, Value::Delete) {
                // Bento: deleting a field of a root nothing has been assigned to yet leaves an empty object.
                if !path.is_empty() && matches!(ctx.new_value, Value::Nothing) {
                    *ctx.new_value = Value::Object(BTreeMap::new().into());
                }
                ctx.new_value.delete_path(path);
                Ok(())
            } else {
                ctx.new_value.set_path(path, val)
            }
        }
    }
}

/// A compiled dynamic field expression with zero or more `${! ... }` interpolations.
#[derive(Clone, Debug)]
pub enum FieldResolver {
    Static(String),
    Query(Box<Expr>),
}

#[derive(Clone)]
pub struct FieldExpression {
    resolvers: Arc<Vec<FieldResolver>>,
    env: Environment,
    named_counters: Arc<Mutex<HashMap<String, i64>>>,
    stateful_counters: Arc<Mutex<HashMap<usize, i64>>>,
    rng_counter: Arc<AtomicU64>,
}

impl std::fmt::Debug for FieldExpression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FieldExpression")
            .field("resolvers", &self.resolvers)
            .finish()
    }
}

impl FieldExpression {
    pub fn new(resolvers: Vec<FieldResolver>, env: Environment) -> Self {
        Self {
            resolvers: Arc::new(resolvers),
            env,
            named_counters: Arc::new(Mutex::new(HashMap::new())),
            stateful_counters: Arc::new(Mutex::new(HashMap::new())),
            rng_counter: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn is_static(&self) -> bool {
        self.resolvers
            .iter()
            .all(|r| matches!(r, FieldResolver::Static(_)))
    }

    pub fn resolve_string(
        &self,
        index: usize,
        batch: &dyn MessageBatch,
    ) -> Result<String, BloblangError> {
        let mut out = String::new();
        for r in self.resolvers.iter() {
            match r {
                FieldResolver::Static(s) => out.push_str(s),
                FieldResolver::Query(q) => {
                    let v = self.eval_query(index, batch, q)?;
                    out.push_str(&v.to_string_coerced());
                }
            }
        }
        Ok(out)
    }

    pub fn resolve_bytes(
        &self,
        index: usize,
        batch: &dyn MessageBatch,
    ) -> Result<Vec<u8>, BloblangError> {
        if self.resolvers.len() == 1 {
            if let FieldResolver::Query(q) = &self.resolvers[0] {
                let v = self.eval_query(index, batch, q)?;
                return Ok(v.to_bytes_coerced());
            }
        }
        Ok(self.resolve_string(index, batch)?.into_bytes())
    }

    fn eval_query(
        &self,
        index: usize,
        batch: &dyn MessageBatch,
        q: &Expr,
    ) -> Result<Value, BloblangError> {
        let mut dummy_root = Value::Nothing;
        let mut vars = HashMap::new();
        let maps = Arc::new(HashMap::new());
        let mut meta_copy = batch.part_meta_all(index);
        let mut ctx = EvalContext {
            env: &self.env,
            maps: &maps,
            vars: &mut vars,
            index,
            batch,
            new_meta: Some(&mut meta_copy),
            new_value: &mut dummy_root,
            cached_root_value: None,
            value_stack: Vec::new(),
            named_values: Vec::new(),
            stack_count: 0,
            max_stacks: self.env.max_map_recursion,
            named_counters: &self.named_counters,
            stateful_counters: &self.stateful_counters,
            rng_counter: &self.rng_counter,
            meta_in_place: false,
        };
        ctx.eval_expr(q).map_err(BloblangError::Runtime)
    }
}

fn byte_text(v: &Value) -> &[u8] {
    match v {
        Value::String(s) => s.as_bytes(),
        Value::Bytes(b) => b,
        _ => &[],
    }
}
