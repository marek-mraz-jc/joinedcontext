use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
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

/// The generator of each `random_int` call site, seeded at its first call.
pub type RngStates = Arc<Mutex<HashMap<usize, crate::gorand::GoRand>>>;

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
    /// The part's bytes; a part holding only a structured value serialises it into an owned copy.
    fn part_bytes(&self, index: usize) -> Cow<'_, [u8]>;
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
    fn part_bytes(&self, _index: usize) -> Cow<'_, [u8]> {
        Cow::Borrowed(&[])
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
    fn part_bytes(&self, index: usize) -> Cow<'_, [u8]> {
        Cow::Borrowed(self.get(index).map(|p| p.raw.as_slice()).unwrap_or(&[]))
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
    rng_states: RngStates,
    reads_other_parts: bool,
}

impl Executor {
    pub fn new(
        annotation: String,
        maps: Arc<HashMap<String, Arc<MapDef>>>,
        statements: Vec<Statement>,
        env: Environment,
    ) -> Self {
        // `from`/`from_all` are the only reads of a part other than the one mapped; read from the
        // parsed tree's Debug text (a method's name prints as `name: "from"`, never a literal).
        let tree = format!("{statements:?}{maps:?}");
        let reads_other_parts =
            tree.contains("name: \"from\"") || tree.contains("name: \"from_all\"");
        Self {
            annotation,
            maps,
            statements: Arc::new(statements),
            reads_other_parts,
            env,
            named_counters: Arc::new(Mutex::new(HashMap::new())),
            stateful_counters: Arc::new(Mutex::new(HashMap::new())),
            rng_states: RngStates::default(),
        }
    }

    /// Whether the mapping may read a batch part other than the one it maps (`from`,
    /// `from_all`), so the caller must keep the whole input batch until every part is mapped.
    pub fn reads_other_parts(&self) -> bool {
        self.reads_other_parts
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
            rng_states: &self.rng_states,
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
    pub rng_states: &'a RngStates,
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
        if let Some(top) = self.value_stack.last() {
            return Ok(top);
        }
        let (batch, index) = (self.batch, self.index);
        match self
            .cached_root_value
            .get_or_insert_with(|| batch.part_structured(index))
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
        if self.stack_count >= self.max_stacks {
            let err = BloblangError::MaxStacksExceeded {
                annotation: format!("map {map_name}"),
                max_stacks: self.max_stacks,
            };
            return Err(err.to_string());
        }
        let Some(map_def) = self.maps.get(map_name).cloned() else {
            return Err(format!("map {map_name} was not found"));
        };
        self.stack_count += 1;

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
                rng_states: self.rng_states,
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
                    // The parser binds a name only inside its lambda, so the lookup finds it.
                    let Some((_, v)) = self.named_values.iter().rev().find(|(k, _)| k == nc) else {
                        return Err(format!("named context {nc} was not found"));
                    };
                    v
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
                // Every caller pushes the context the lambda captures before evaluating it.
                let Some(captured) = self.value_stack.pop() else {
                    return Err(format!(
                        "failed to capture context {name}: context was undefined"
                    ));
                };
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

    // Both walks stay inside `data`: a forward start is in 0..=len and its end at most len; a
    // backward start is at most len - 1 and its end at least -1, so the walk ends at index 0.
    if step_val > 0 {
        data[start_idx as usize..end_idx.max(start_idx) as usize]
            .iter()
            .step_by(step_val as usize)
            .cloned()
            .collect()
    } else if start_idx <= end_idx {
        Vec::new()
    } else {
        data[(end_idx + 1) as usize..=start_idx as usize]
            .iter()
            .rev()
            .step_by(step_val.unsigned_abs() as usize)
            .cloned()
            .collect()
    }
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
                // Gt, Gte, Lt, Lte: Go compares two strings byte by byte, anything else as float64.
                _ => {
                    let ord = match (&lv, &rv) {
                        (
                            Value::String(_) | Value::Bytes(_),
                            Value::String(_) | Value::Bytes(_),
                        ) => Some(byte_text(&lv).cmp(byte_text(&rv))),
                        _ => {
                            let a = lv.as_f64().ok_or_else(mismatch)?;
                            let b = rv.as_f64().ok_or_else(mismatch)?;
                            a.partial_cmp(&b)
                        }
                    };
                    Ok(Value::Bool(ord.is_some_and(|o| match op {
                        BinaryOp::Gt => o.is_gt(),
                        BinaryOp::Gte => o.is_ge(),
                        BinaryOp::Lt => o.is_lt(),
                        _ => o.is_le(),
                    })))
                }
            }
        }
    }
}

/// Bento's `==` (`compareOp` in internal/bloblang/query/arithmetic.go): the left operand's type picks the
/// coercion, so `true == 7` holds (7 reads as a bool) while `1 == true` does not.
fn compare_eq_or_bytes(lv: &Value, rv: &Value) -> bool {
    match lv {
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
        Value::Timestamp(ts) => Some(crate::time_format::format_rfc3339_auto_si(ts, true)),
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
    rng_states: RngStates,
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
            rng_states: RngStates::default(),
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
            rng_states: &self.rng_states,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    type R = Result<(), Box<dyn std::error::Error>>;

    fn exec(mapping: &str) -> Executor {
        Environment::new()
            .compile_mapping(mapping)
            .expect("the mapping compiles")
    }

    /// The mapping's result on `input` (JSON text) as JSON, or its error text.
    fn run(mapping: &str, input: &str) -> Result<serde_json::Value, String> {
        match exec(mapping).execute_bytes(input.as_bytes()) {
            Ok(Some(v)) => Ok(v.to_json_value()),
            Ok(None) => Ok(json!("<deleted>")),
            Err(e) => Err(e.to_string()),
        }
    }

    fn err(mapping: &str, input: &str) -> String {
        run(mapping, input).expect_err(mapping)
    }

    fn part(raw: &str) -> SimplePartView {
        SimplePartView::from_bytes(raw)
    }

    #[test]
    fn an_empty_batch_has_no_part_to_read() {
        let b = EmptyBatch;
        assert_eq!(b.len(), 0);
        assert!(b.is_empty());
        assert!(b.part_bytes(0).is_empty());
        assert_eq!(
            b.part_structured(0).err().as_deref(),
            Some("message is empty")
        );
        assert!(b.part_meta_get(0, "k").is_none());
        assert!(b.part_meta_all(0).is_empty());
        assert!(b.part_error(0).is_none());
    }

    #[test]
    fn a_part_array_reads_its_parts_and_answers_empty_past_the_end() {
        let mut p = part(r#"{"a":1}"#);
        p.meta.insert("k".into(), Value::String("v".into()));
        p.error = Some(DetailedErrorInfo {
            message: "boom".into(),
            ..Default::default()
        });
        let b = [p];
        assert_eq!(MessageBatch::len(&b), 1);
        assert!(!MessageBatch::is_empty(&b));
        assert_eq!(&*b.part_bytes(0), br#"{"a":1}"#);
        assert!(b.part_bytes(1).is_empty());
        assert_eq!(
            b.part_structured(1).err().as_deref(),
            Some("message is empty")
        );
        assert_eq!(b.part_meta_get(0, "k"), Some(Value::String("v".into())));
        assert!(b.part_meta_get(1, "k").is_none());
        assert_eq!(b.part_meta_all(0).len(), 1);
        assert!(b.part_meta_all(1).is_empty());
        assert_eq!(b.part_error(0).map(|e| e.message).as_deref(), Some("boom"));
        assert!(b.part_error(1).is_none());
        assert_eq!(
            [part("")].part_structured(0).err().as_deref(),
            Some("message is empty")
        );
        let bad = [part("{nope")].part_structured(0).err().unwrap_or_default();
        assert!(bad.starts_with("parse as json: "), "{bad}");
    }

    #[test]
    fn btree_metadata_sets_reads_deletes_and_clears() {
        let mut m: BTreeMap<String, Value> = BTreeMap::new();
        m.meta_set("a".into(), Value::Int(1));
        m.meta_set("b".into(), Value::Int(2));
        assert_eq!(PartMetaMut::meta_get(&m, "a"), Some(Value::Int(1)));
        m.meta_delete("a");
        assert_eq!(PartMetaMut::meta_get(&m, "a"), None);
        assert_eq!(m.meta_all().len(), 1);
        m.meta_clear();
        assert!(m.meta_all().is_empty());
    }

    #[test]
    fn executor_reports_whether_it_reads_other_parts_and_its_annotation() {
        assert!(exec("root = this.from(0)").reads_other_parts());
        assert!(exec("root = this.from_all()").reads_other_parts());
        assert!(!exec("root = this.from_x").reads_other_parts());
        assert_eq!(exec("root = 1").annotation(), "");
    }

    #[test]
    fn a_batch_of_two_parts_has_length_two_and_reads_each() {
        let b = [part("1"), part("2")];
        assert_eq!(MessageBatch::len(&b), 2);
        assert_eq!(&*b.part_bytes(1), b"2");
    }

    #[test]
    fn targets_follow_named_maps_and_list_every_assignment() {
        let e = exec(
            "map m {\n root = this.inner\n}\nroot.a = this.apply(\"m\")\nmeta k = 1\nlet v = 2",
        );
        let queries: Vec<String> = e.query_targets().iter().map(|t| t.path.join(".")).collect();
        assert!(queries.contains(&"inner".to_string()), "{queries:?}");
        let assigned: Vec<String> = e
            .assignment_targets()
            .iter()
            .map(|t| t.path.join("."))
            .collect();
        assert_eq!(assigned.len(), 3, "{assigned:?}");
        assert!(assigned.contains(&"a".to_string()), "{assigned:?}");
    }

    #[test]
    fn a_caught_missing_map_gives_its_stack_level_back() -> R {
        // Two levels allowed: a missing map at level 1 must leave room for m and its leaf.
        let e = Environment::new()
            .with_max_map_recursion(2)
            .compile_mapping(
                "map leaf {\n root = 1\n}\nmap m {\n root = this.apply(\"leaf\")\n}\n\
             root.a = this.apply(\"nope\").catch(0)\nroot.b = this.apply(\"m\")",
            )?;
        let out = e.execute_bytes(b"{}")?;
        assert_eq!(
            out.map(|v| v.to_json_value()),
            Some(json!({"a": 0, "b": 1}))
        );
        // One level allowed: each finished map gives its level back for the next.
        let twice = Environment::new().with_max_map_recursion(1).compile_mapping(
            "map leaf {\n root = 1\n}\nroot.a = this.apply(\"leaf\")\nroot.b = this.apply(\"leaf\")",
        )?;
        let out = twice.execute_bytes(b"{}")?;
        assert_eq!(
            out.map(|v| v.to_json_value()),
            Some(json!({"a": 1, "b": 1}))
        );
        Ok(())
    }

    #[test]
    fn query_part_answers_the_bool_and_refuses_a_deletion_or_another_type() {
        let mut p = part(r#"{"n":3}"#);
        p.meta.insert("m".into(), Value::String("x".into()));
        let b = [p];
        assert!(exec("root = this.n > 2").query_part(0, &b).unwrap_or(false));
        assert!(exec(r#"root = @m == "x""#)
            .query_part(0, &b)
            .unwrap_or(false));
        assert!(!exec("root = this.n > 5").query_part(0, &b).unwrap_or(true));
        let del = exec("root = deleted()")
            .query_part(0, &b)
            .err()
            .map(|e| e.to_string());
        assert_eq!(
            del.as_deref(),
            Some("query mapping resulted in deleted message, expected a boolean value")
        );
        let other = exec("root = this.n")
            .query_part(0, &b)
            .err()
            .map(|e| e.to_string());
        assert_eq!(
            other.as_deref(),
            Some("mapping: expected bool value, got number (3)")
        );
    }

    #[test]
    fn mutate_part_reads_metadata_as_written_so_far_and_map_part_reads_the_input() -> R {
        let mut p = part("{}");
        p.meta.insert("m".into(), Value::String("in".into()));
        let b = [p];
        let e = exec(
            r#"meta m = "out"
root.v = metadata("m")"#,
        );
        let mut meta = b.part_meta_all(0);
        let mutated = e.mutate_part(0, &b, &mut meta, None)?;
        assert_eq!(
            mutated.map(|v| v.to_json_value()),
            Some(json!({"v": "out"}))
        );
        let mut meta = b.part_meta_all(0);
        let mapped = e.map_part(0, &b, &mut meta, None)?;
        assert_eq!(mapped.map(|v| v.to_json_value()), Some(json!({"v": "in"})));
        let in_place = exec(
            r#"meta m = "w"
root = metadata("m")"#,
        )
        .execute_bytes_in_place(b"{}")?;
        assert_eq!(in_place.map(|v| v.to_json_value()), Some(json!("w")));
        Ok(())
    }

    #[test]
    fn this_on_unstructured_input_names_the_reference_in_the_error() {
        let whole = err("root = this", "not json");
        assert!(
            whole.contains("unable to reference message as structured (with 'this'): "),
            "{whole}"
        );
        let field = err("root = this.a.b", "not json");
        assert!(
            field.contains("unable to reference message as structured (with 'this.a.b'): "),
            "{field}"
        );
    }

    #[test]
    fn object_literal_keys_are_strings_or_bytes_and_errors_name_the_key() {
        assert_eq!(
            run(
                r#"root = {this.k.bytes(): 1, "b": deleted(), "c": this.nothing_here}"#,
                r#"{"k":"a"}"#
            ),
            Ok(json!({"a": 1, "c": null}))
        );
        assert_eq!(
            err("root = {this.n: 1}", r#"{"n":5}"#),
            "failed assignment (line 1): mapping returned invalid key type: number"
        );
        let key = err(r#"root = {this.a.uppercase(): 1}"#, r#"{"a":1}"#);
        assert!(key.contains("failed to resolve key: "), "{key}");
        let val = err(r#"root = {"k": this.a.uppercase()}"#, r#"{"a":1}"#);
        assert!(val.contains("failed to resolve 'k' value: "), "{val}");
    }

    #[test]
    fn array_literals_drop_deleted_and_nothing_items() {
        assert_eq!(run("root = [1, deleted(), 2]", "{}"), Ok(json!([1, 2])));
    }

    #[test]
    fn variables_and_named_contexts_resolve_or_name_what_is_missing() {
        assert_eq!(
            err("root = $nope", "{}"),
            "failed assignment (line 1): variable 'nope' undefined"
        );
        assert_eq!(
            run("let v = 1\nlet v = deleted()\nroot = $v | 7", "{}"),
            Ok(json!(7))
        );
        assert_eq!(
            run("root = this.a.(x -> x + 1)", r#"{"a":1}"#),
            Ok(json!(2))
        );
        assert_eq!(
            run("root = [1,2].map_each(x -> x + 1)", "{}"),
            Ok(json!([2, 3]))
        );
        assert_eq!(
            run("root = [1,2].map_each(_ -> 5)", "{}"),
            Ok(json!([5, 5]))
        );
        assert_eq!(
            run(
                "root = [1].map_each(x -> [5].map_each(_ -> 0).append(x))",
                "{}"
            ),
            Ok(json!([[0, 1]]))
        );
    }

    #[test]
    fn index_reads_strings_bytes_and_arrays_from_either_end_and_refuses_out_of_bounds() {
        assert_eq!(run(r#"root = "héllo"[1]"#, "{}"), Ok(json!("é")));
        assert_eq!(run(r#"root = "abc"[-1]"#, "{}"), Ok(json!("c")));
        assert_eq!(run(r#"root = "abc".bytes()[-3]"#, "{}"), Ok(json!(97)));
        assert_eq!(run("root = [1,2,3][-1]", "{}"), Ok(json!(3)));
        assert_eq!(run(r#"root = "abc"[0]"#, "{}"), Ok(json!("a")));
        assert_eq!(run(r#"root = "abc".bytes()[0]"#, "{}"), Ok(json!(97)));
        assert_eq!(run("root = [7][0]", "{}"), Ok(json!(7)));
        for m in [
            r#"root = "abc"[3]"#,
            r#"root = "abc"[-4]"#,
            r#"root = "abc".bytes()[3]"#,
            r#"root = "abc".bytes()[-4]"#,
            "root = [1][1]",
            "root = [1][-2]",
        ] {
            let e = err(m, "{}");
            assert!(e.contains("out of bounds for length"), "{m}: {e}");
        }
        let num = err("root = this.n[0]", r#"{"n":5}"#);
        assert!(
            num.contains("expected array or string value, got number"),
            "{num}"
        );
        assert!(err(r#"root = [1]["x"]"#, "{}").contains("index: "));
    }

    #[test]
    fn slices_take_bounds_from_either_end_and_steps_both_ways() {
        let cases = [
            (r#"root = "héllo"[1:3]"#, json!("él")),
            (r#"root = "abc".bytes()[1:]"#, json!("YmM=")),
            ("root = [1,2,3,4][:2]", json!([1, 2])),
            ("root = [1,2,3,4][-2:]", json!([3, 4])),
            ("root = [1,2,3,4][:-1]", json!([1, 2, 3])),
            ("root = [1,2,3,4][::2]", json!([1, 3])),
            ("root = [1,2,3,4][1:10:2]", json!([2, 4])),
            ("root = [1,2,3,4][::-1]", json!([4, 3, 2, 1])),
            ("root = [1,2,3,4][10::-2]", json!([4, 2])),
            ("root = [1,2,3,4][-1:0:-1]", json!([4, 3, 2])),
            ("root = [1,2,3,4][-1:-3]", json!([])),
            ("root = [1,2,3,4][5:9]", json!([])),
            ("root = [1,2,3,4][1:1]", json!([])),
        ];
        for (m, want) in cases {
            assert_eq!(run(m, "{}"), Ok(want), "{m}");
        }
        assert!(err("root = [1,2][::0]", "{}").contains("slice step cannot be zero"));
        assert!(err("root = [1,2][2:1]", "{}")
            .contains("lower slice bound (2) must be lower than upper (1)"));
        assert!(err(r#"root = [1,2]["a":]"#, "{}").contains("slice start bound: "));
        assert!(err(r#"root = [1,2][:"a"]"#, "{}").contains("slice end bound: "));
        assert!(err(r#"root = [1,2][::"a"]"#, "{}").contains("slice step: "));
        let num = err("root = this.n[0:1]", r#"{"n":5}"#);
        assert!(
            num.contains("expected array or string value, got number"),
            "{num}"
        );
    }

    #[test]
    fn slice_items_clamps_and_walks_each_direction() {
        let d = [1, 2, 3, 4, 5];
        assert_eq!(slice_items(&d, None, None, None), d.to_vec());
        assert_eq!(slice_items(&d, Some(-2), None, None), vec![4, 5]);
        assert_eq!(slice_items(&d, Some(9), None, None), Vec::<i32>::new());
        assert_eq!(slice_items(&d, None, Some(9), None), d.to_vec());
        assert_eq!(slice_items(&d, Some(4), Some(1), Some(-1)), vec![5, 4, 3]);
        assert_eq!(slice_items(&d, Some(9), Some(-9), Some(-2)), vec![5, 3, 1]);
        assert_eq!(slice_items(&d, None, None, Some(-1)), vec![5, 4, 3, 2, 1]);
        assert_eq!(slice_items(&d, Some(-9), Some(9), Some(3)), vec![1, 4]);
        assert_eq!(slice_items(&d, Some(1), Some(4), Some(2)), vec![2, 4]);
        assert_eq!(
            slice_items::<i32>(&[], None, None, Some(-1)),
            Vec::<i32>::new()
        );
    }

    #[test]
    fn not_negates_a_bool_and_refuses_another_type() {
        assert_eq!(run("root = !this.b", r#"{"b":false}"#), Ok(json!(true)));
        let e = err("root = !this.n", r#"{"n":1}"#);
        assert!(e.contains("expected bool value, got number"), "{e}");
    }

    #[test]
    fn binary_operators_follow_bento() {
        let cases = [
            (r#"root = "a" + "b""#, json!("ab")),
            (r#"root = "a".bytes() + "b""#, json!("ab")),
            ("root = 1 + 2", json!(3)),
            ("root = 1 + 2.5", json!(3.5)),
            ("root = 5 - 2", json!(3)),
            ("root = 5.5 - 2", json!(3.5)),
            ("root = 3 * 4", json!(12)),
            ("root = 1.5 * 2", json!(3)),
            ("root = 7 / 2", json!(3.5)),
            ("root = 7 % 3", json!(1)),
            ("root = 7.9 % 3", json!(1)),
            ("root = 1 == 1.0", json!(true)),
            ("root = 1 != 2", json!(true)),
            ("root = true == 7", json!(true)),
            ("root = false == 0", json!(true)),
            ("root = 1 == true", json!(false)),
            ("root = 1 == \"1\"", json!(false)),
            (r#"root = "a" == "a".bytes()"#, json!(true)),
            ("root = null == null", json!(true)),
            ("root = null == 1", json!(false)),
            ("root = true == \"x\"", json!(false)),
            ("root = 1 == null", json!(false)),
            ("root = [1, 2] == [1, 2]", json!(true)),
            ("root = [1, 2] == [1]", json!(false)),
            ("root = [1] == 1", json!(false)),
            (r#"root = {"a": 1} == {"a": 1}"#, json!(true)),
            (r#"root = {"a": 1} == {"b": 1}"#, json!(false)),
            (r#"root = {"a": 1} == 1"#, json!(false)),
            (r#"root = "b" > "a""#, json!(true)),
            (r#"root = "b" >= "b""#, json!(true)),
            (r#"root = "a" < "b""#, json!(true)),
            (r#"root = "b" <= "a""#, json!(false)),
            ("root = 2 > 1", json!(true)),
            ("root = 2 >= 3", json!(false)),
            ("root = 2 < 1.5", json!(false)),
            ("root = 2 <= 2", json!(true)),
            ("root = 2 < 2", json!(false)),
            (r#"root = "b" < "b""#, json!(false)),
            ("root = 9007199254740993 == 9007199254740992", json!(false)),
            ("root = true && false", json!(false)),
            ("root = false && this.nope.uppercase()", json!(false)),
            ("root = true && true", json!(true)),
            ("root = true || this.nope.uppercase()", json!(true)),
            ("root = false || false", json!(false)),
            ("root = null | 3", json!(3)),
            ("root = this.x.uppercase() | 4", json!(4)),
            ("root = 2 | 4", json!(2)),
        ];
        for (m, want) in cases {
            assert_eq!(run(m, r#"{"x":1}"#), Ok(want), "{m}");
        }
        assert_eq!(
            run("root = this.a == this.b", r#"{"a":[1.0],"b":[1]}"#),
            Ok(json!(true))
        );
        assert!(run("root = this.a == this.b", r#"{"a":1e400,"b":1}"#).is_ok());
        let errors = [
            ("root = 1 / 0", "attempted to divide by zero"),
            ("root = 1 % 0", "attempted to divide by zero"),
            (r#"root = "a" - 1"#, "cannot subtract types string (from string literal) and number (from number literal)"),
            (r#"root = 1.5 - "a""#, "cannot subtract types"),
            (r#"root = "a" * 1.5"#, "cannot multiply types"),
            (r#"root = "a" + 1"#, "cannot add types"),
            (r#"root = "a" / 1"#, "cannot divide types"),
            (r#"root = 1 / "a""#, "cannot divide types"),
            (r#"root = "a" % 1"#, "cannot modulo types"),
            (r#"root = 1 % "a""#, "cannot modulo types"),
            (r#"root = "nan".number() % 3"#, "cannot modulo types"),
            (r#"root = "a" > 1"#, "cannot compare types"),
            (r#"root = 1 > "a""#, "cannot compare types"),
            (r#"root = "x" && true"#, "expected bool value"),
            (r#"root = true && "x""#, "expected bool value"),
            (r#"root = "x" || true"#, "expected bool value"),
            (r#"root = false || "x""#, "expected bool value"),
            ("root = this.nope.uppercase() && true", "expected"),
        ];
        for (m, want) in errors {
            let e = err(m, "{}");
            assert!(e.contains(want), "{m}: {e}");
        }
    }

    #[test]
    fn a_pipe_keeps_the_max_stacks_error_of_its_left_side() -> R {
        let e = Environment::new()
            .with_max_map_recursion(0)
            .compile_mapping("map m {\n root = 1\n}\nroot = this.apply(\"m\") | 1")?
            .execute_bytes(b"{}");
        assert!(e
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default()
            .contains("entering map m exceeded maximum allowed stacks of 0"));
        // Inside a map the error comes wrapped by its statement, and the pipe falls back.
        let wrapped = Environment::new()
            .with_max_map_recursion(1)
            .compile_mapping("map m2 {\n root = 1\n}\nmap m {\n root = this.apply(\"m2\")\n}\nroot = this.apply(\"m\") | 5")?
            .execute_bytes(b"{}")?;
        assert_eq!(wrapped.map(|v| v.to_json_value()), Some(json!(5)));
        Ok(())
    }

    #[test]
    fn named_maps_refuse_an_unknown_name_and_a_lowered_stack_limit() -> R {
        assert!(err(r#"root = this.apply("missing")"#, "{}").contains("map missing was not found"));
        let env = Environment::new().with_max_map_recursion(1);
        let e = env.compile_mapping("map a {\n root = this.apply(\"b\")\n}\nmap b {\n root = 1\n}\nroot = this.apply(\"a\")")?;
        let msg = e
            .execute_bytes(b"{}")
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(
            msg.contains("entering map b exceeded maximum allowed stacks of 1"),
            "{msg}"
        );
        let bad = exec("map a {\n root = 1 / 0\n}\nroot = this.apply(\"a\")").execute_bytes(b"{}");
        assert!(bad
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default()
            .contains("divide by zero"));
        Ok(())
    }

    #[test]
    fn if_expressions_pick_a_branch_and_refuse_a_non_bool_condition() {
        let m = "root = if this.n == 1 { \"one\" } else if this.n == 2 { \"two\" } else if this.n == null { \"null\" } else { \"many\" }";
        assert_eq!(run(m, r#"{"n":1}"#), Ok(json!("one")));
        assert_eq!(run(m, r#"{"n":2}"#), Ok(json!("two")));
        assert_eq!(run(m, r#"{"n":3}"#), Ok(json!("many")));
        assert_eq!(
            run("root.a = if null { 1 } else if null { 2 }", "{}"),
            Ok(json!(null))
        );
        assert_eq!(
            run("root = if this.n == 1 { 1 }", r#"{"n":2}"#),
            Ok(json!(null))
        );
        let e = err("root = if 5 { 1 }", "{}");
        assert!(
            e.contains("resolved to a non-boolean value 5 (number)"),
            "{e}"
        );
        let e = err("root = if false { 1 } else if \"x\" { 2 }", "{}");
        assert!(
            e.contains("resolved to a non-boolean value \"x\" (string)"),
            "{e}"
        );
        let e = err("root = if this.a.uppercase() { 1 }", r#"{"a":1}"#);
        assert!(e.contains("failed to check if condition: "), "{e}");
        let e = err(
            "root = if false { 1 } else if this.a.uppercase() { 2 }",
            r#"{"a":1}"#,
        );
        assert!(e.contains("failed to check if condition 1: "), "{e}");
    }

    #[test]
    fn match_tries_literals_conditions_and_wildcards_in_order() {
        let m = "root = match this.v { 1 => \"one\", this > 5 => \"big\", _ => \"other\" }";
        assert_eq!(run(m, r#"{"v":1}"#), Ok(json!("one")));
        assert_eq!(run(m, r#"{"v":9}"#), Ok(json!("big")));
        assert_eq!(run(m, r#"{"v":3}"#), Ok(json!("other")));
        assert_eq!(
            run("root = match { this.v == 1 => \"one\" }", r#"{"v":1}"#),
            Ok(json!("one"))
        );
        assert_eq!(
            run("root = match \"x\" { \"y\" => 1 }", r#"{"a":1}"#),
            Ok(json!(null))
        );
        assert_eq!(run("root = match { _ => 2 }", "not json"), Ok(json!(2)));
        let e = err("root = match 1 { this.uppercase() => 1 }", "{}");
        assert!(e.contains("failed to check match case 0: "), "{e}");
    }

    #[test]
    fn statements_wrap_errors_with_the_outer_line_and_refuse_non_bool_conditions() {
        let m = "root = {}\nif this.n > 0 {\n  root.a = 1\n  root.b = 1 / 0\n}";
        let e = err(m, r#"{"n":1}"#);
        assert!(e.starts_with("failed assignment (line 2): "), "{e}");
        assert_eq!(
            run(
                "if this.n > 5 {\n root = 1\n} else if null {\n root = 2\n} else {\n root = 3\n}",
                r#"{"n":1}"#
            ),
            Ok(json!(3))
        );
        assert_eq!(
            run("root = 0\nif false {\n root = 1\n}", "{}"),
            Ok(json!(0))
        );
        let e = err("if 3 {\n root = 1\n}", "{}");
        assert!(
            e.contains("(line 1): ") && e.contains("resolved to a non-boolean value 3 (number)"),
            "{e}"
        );
        let e = err("if this.a.uppercase() {\n root = 1\n}", r#"{"a":1}"#);
        assert!(e.contains("failed to check if condition 1: "), "{e}");
        let e = err(
            "map m {\n root = 1\n}\nif true {\n root = this.apply(\"m\").apply(\"nope\")\n}",
            "{}",
        );
        assert!(e.contains("map nope was not found"), "{e}");
    }

    #[test]
    fn assignments_write_variables_metadata_and_root_paths() -> R {
        let b = [part("{}")];
        let e = exec("meta a = 1\nmeta b = 2\nmeta a = deleted()\nroot.m = @\nmeta = {\"c\": 3}\nroot.n = @\nmeta = deleted()\nroot.o = @\nroot.p = @c");
        let mut meta = BTreeMap::new();
        let out = e.map_part(0, &b, &mut meta, None)?;
        assert_eq!(
            out.map(|v| v.to_json_value()),
            Some(json!({"m": {"b": 2}, "n": {"c": 3}, "o": {}, "p": null}))
        );
        assert!(err("meta = 5", "{}")
            .contains("setting root meta object requires object value, received: number"));
        assert_eq!(run("root.a = deleted()", "{}"), Ok(json!({})));
        assert_eq!(
            run("root.a = 1\nroot.b = root.a + 1", "{}"),
            Ok(json!({"a": 1, "b": 2}))
        );
        assert_eq!(run("root = deleted()", "{}"), Ok(json!("<deleted>")));
        assert!(run("root = 1\nroot.a = 2", "{}").is_err());
        let e = exec("root = @").map_part(0, &b, &mut BTreeMap::new(), Some(Value::Int(1)))?;
        assert_eq!(e.map(|v| v.to_json_value()), Some(json!({})));
        Ok(())
    }

    #[test]
    fn field_expressions_interpolate_queries_and_report_static_ones() -> R {
        let env = Environment::new();
        let mut p = part(r#"{"a":"x"}"#);
        p.meta.insert("m".into(), Value::String("meta".into()));
        let b = [p];
        let f = env.compile_field("id-${! this.a }-${! @m }")?;
        assert!(!f.is_static());
        assert_eq!(f.resolve_string(0, &b)?, "id-x-meta");
        assert_eq!(f.resolve_bytes(0, &b)?, b"id-x-meta");
        assert!(format!("{f:?}").starts_with("FieldExpression { resolvers: ["));
        let two = env.compile_field("${! this.a }-${! @m }")?;
        assert_eq!(two.resolve_bytes(0, &b)?, b"x-meta");
        let one = env.compile_field("${! this.a }")?;
        assert_eq!(one.resolve_bytes(0, &b)?, b"x");
        let fixed = env.compile_field("plain")?;
        assert!(fixed.is_static());
        assert_eq!(fixed.resolve_bytes(0, &b)?, b"plain");
        let bad = env.compile_field("${! this.a.number() }")?;
        assert!(bad.resolve_bytes(0, &b).is_err());
        assert!(bad.resolve_string(0, &b).is_err());
        Ok(())
    }

    #[test]
    fn a_named_map_reads_the_input_part_metadata() {
        let mut p = part("{}");
        p.meta.insert("k".into(), Value::String("v".into()));
        let b = [p];
        let e = exec("map m {\n root.one = @k\n root.all = @\n}\nroot = this.apply(\"m\")");
        let out = e.map_part(0, &b, &mut BTreeMap::new(), None);
        assert_eq!(
            out.ok().flatten().map(|v| v.to_json_value()),
            Some(json!({"one": "v", "all": {"k": "v"}}))
        );
    }

    #[test]
    fn icompare_reads_the_right_side_as_the_left_sides_type() {
        assert!(icompare(&Value::Bool(true), &Value::Bool(true)));
        assert!(!icompare(&Value::Bool(true), &Value::Bool(false)));
        assert!(!icompare(&Value::Delete, &Value::Delete));
        assert!(!icompare(&Value::Nothing, &Value::Null));
        assert!(compare_eq_or_bytes(&Value::Null, &Value::Null));
        assert_eq!(run("root = [true] == [true]", "{}"), Ok(json!(true)));
        assert_eq!(
            run("root = this.a == this.b", r#"{"a":1.10,"b":1.1}"#),
            Ok(json!(true))
        );
        assert_eq!(run("root = this.a > 1", r#"{"a":1.10}"#), Ok(json!(true)));
    }

    #[test]
    fn comparisons_with_nan_are_false_and_strings_compare_bytewise() {
        let nan = r#"root = "nan".number()"#;
        assert!(run(nan, "{}").is_ok());
        for m in [
            "root = (\"nan\".number()) > 1",
            "root = (\"nan\".number()) >= 1",
            "root = (\"nan\".number()) < 1",
            "root = (\"nan\".number()) <= 1",
        ] {
            assert_eq!(run(m, "{}"), Ok(json!(false)), "{m}");
        }
        assert_eq!(run(r#"root = "é" > "z""#, "{}"), Ok(json!(true)));
        assert_eq!(run(r#"root = "a".bytes() < "b""#, "{}"), Ok(json!(true)));
    }

    #[test]
    fn byte_text_reads_strings_and_bytes_only() {
        assert_eq!(byte_text(&Value::String("a".into())), b"a");
        assert_eq!(byte_text(&Value::Bytes(b"b".to_vec())), b"b");
        assert!(byte_text(&Value::Int(1)).is_empty());
    }
}
