use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::config::{ComponentConfig, StreamConfig};
use crate::error::CoreError;
use crate::pipeline::ObservedProcessor;
use crate::traits::{
    Buffer, Cache, Input, Output, ProcContext, Processor, RateLimit, StreamContext,
};

pub type ProcessorCtor = Arc<
    dyn Fn(&ComponentConfig, &ProcContext) -> Result<Arc<dyn Processor>, CoreError> + Send + Sync,
>;
pub type InputCtor = Arc<
    dyn Fn(&ComponentConfig, &StreamContext) -> Result<Box<dyn Input>, CoreError> + Send + Sync,
>;
pub type OutputCtor = Arc<
    dyn Fn(&ComponentConfig, &StreamContext) -> Result<Arc<dyn Output>, CoreError> + Send + Sync,
>;
pub type CacheCtor = Arc<
    dyn Fn(&ComponentConfig, &StreamContext) -> Result<Arc<dyn Cache>, CoreError> + Send + Sync,
>;
pub type RateLimitCtor = Arc<
    dyn Fn(&ComponentConfig, &StreamContext) -> Result<Arc<dyn RateLimit>, CoreError> + Send + Sync,
>;
pub type BufferCtor = Arc<
    dyn Fn(&ComponentConfig, &StreamContext) -> Result<Arc<dyn Buffer>, CoreError> + Send + Sync,
>;

/// A component being built: children built while its constructor runs get their Bento config
/// path from it (constructors are synchronous, so a thread-local stack follows them exactly).
struct BuildFrame {
    path: String,
    component_type: String,
    children: usize,
}

thread_local! {
    static BUILD_FRAMES: RefCell<Vec<BuildFrame>> = const { RefCell::new(Vec::new()) };
}

/// Pops the frame pushed for a constructor, also when it fails or panics.
struct FrameGuard;

impl Drop for FrameGuard {
    fn drop(&mut self) {
        BUILD_FRAMES.with(|f| {
            f.borrow_mut().pop();
        });
    }
}

fn push_frame(path: &str, component_type: &str) -> FrameGuard {
    BUILD_FRAMES.with(|f| {
        f.borrow_mut().push(BuildFrame {
            path: path.to_string(),
            component_type: component_type.to_string(),
            children: 0,
        });
    });
    FrameGuard
}

/// The Bento path of the next input (`kind == "input"`) or output child of the component being
/// built, or `default` for a top-level component.
fn next_child_path(kind: &str, default: &str) -> String {
    BUILD_FRAMES.with(|f| {
        let mut frames = f.borrow_mut();
        let Some(top) = frames.last_mut() else {
            return default.to_string();
        };
        let n = top.children;
        top.children += 1;
        let ty = top.component_type.as_str();
        let seg = match (kind, ty) {
            ("output", "fallback") => format!("fallback.{n}"),
            ("output", "switch") => format!("switch.cases.{n}.output"),
            ("output", "retry" | "drop_on" | "drop" | "resource") => format!("{ty}.output"),
            ("input", "read_until" | "resource") => format!("{ty}.input"),
            ("input", _) => format!("{ty}.inputs.{n}"),
            (_, _) => format!("{ty}.outputs.{n}"),
        };
        format!("{}.{seg}", top.path)
    })
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LintReport {
    pub lint_errors: Vec<String>,
    pub linting_errors: Vec<String>,
}

impl LintReport {
    pub fn is_ok(&self) -> bool {
        self.lint_errors.is_empty()
    }

    pub fn add_error(&mut self, msg: impl Into<String>) {
        let s = msg.into();
        self.lint_errors.push(s.clone());
        self.linting_errors.push(s);
    }
}

/// Plugin registry mapping component names to constructors (`Section 5 Modularity`).
///
/// The engine never matches on component names; adding a new processor, input, output,
/// cache, rate limit, or buffer is a single registration call.
#[derive(Default)]
pub struct Registry {
    processors: RwLock<HashMap<String, ProcessorCtor>>,
    inputs: RwLock<HashMap<String, InputCtor>>,
    outputs: RwLock<HashMap<String, OutputCtor>>,
    caches: RwLock<HashMap<String, CacheCtor>>,
    rate_limits: RwLock<HashMap<String, RateLimitCtor>>,
    buffers: RwLock<HashMap<String, BufferCtor>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_processor<F>(&self, name: impl Into<String>, ctor: F)
    where
        F: Fn(&ComponentConfig, &ProcContext) -> Result<Arc<dyn Processor>, CoreError>
            + Send
            + Sync
            + 'static,
    {
        self.processors
            .write()
            .unwrap()
            .insert(name.into(), Arc::new(ctor));
    }

    pub fn register_input<F>(&self, name: impl Into<String>, ctor: F)
    where
        F: Fn(&ComponentConfig, &StreamContext) -> Result<Box<dyn Input>, CoreError>
            + Send
            + Sync
            + 'static,
    {
        self.inputs
            .write()
            .unwrap()
            .insert(name.into(), Arc::new(ctor));
    }

    pub fn register_output<F>(&self, name: impl Into<String>, ctor: F)
    where
        F: Fn(&ComponentConfig, &StreamContext) -> Result<Arc<dyn Output>, CoreError>
            + Send
            + Sync
            + 'static,
    {
        self.outputs
            .write()
            .unwrap()
            .insert(name.into(), Arc::new(ctor));
    }

    pub fn register_cache<F>(&self, name: impl Into<String>, ctor: F)
    where
        F: Fn(&ComponentConfig, &StreamContext) -> Result<Arc<dyn Cache>, CoreError>
            + Send
            + Sync
            + 'static,
    {
        self.caches
            .write()
            .unwrap()
            .insert(name.into(), Arc::new(ctor));
    }

    pub fn register_rate_limit<F>(&self, name: impl Into<String>, ctor: F)
    where
        F: Fn(&ComponentConfig, &StreamContext) -> Result<Arc<dyn RateLimit>, CoreError>
            + Send
            + Sync
            + 'static,
    {
        self.rate_limits
            .write()
            .unwrap()
            .insert(name.into(), Arc::new(ctor));
    }

    pub fn register_buffer<F>(&self, name: impl Into<String>, ctor: F)
    where
        F: Fn(&ComponentConfig, &StreamContext) -> Result<Arc<dyn Buffer>, CoreError>
            + Send
            + Sync
            + 'static,
    {
        self.buffers
            .write()
            .unwrap()
            .insert(name.into(), Arc::new(ctor));
    }

    /// Registered component names per kind, keyed and sorted as `bento list --format json` prints them.
    pub fn component_names(&self) -> std::collections::BTreeMap<&'static str, Vec<String>> {
        fn sorted<V>(m: &RwLock<HashMap<String, V>>) -> Vec<String> {
            let mut v: Vec<String> = m.read().unwrap().keys().cloned().collect();
            v.sort();
            v
        }
        std::collections::BTreeMap::from([
            ("buffers", sorted(&self.buffers)),
            ("caches", sorted(&self.caches)),
            ("inputs", sorted(&self.inputs)),
            ("outputs", sorted(&self.outputs)),
            ("processors", sorted(&self.processors)),
            ("rate-limits", sorted(&self.rate_limits)),
        ])
    }

    pub fn has_processor(&self, name: &str) -> bool {
        self.processors.read().unwrap().contains_key(name)
    }

    pub fn has_input(&self, name: &str) -> bool {
        self.inputs.read().unwrap().contains_key(name)
    }

    pub fn has_output(&self, name: &str) -> bool {
        self.outputs.read().unwrap().contains_key(name)
    }

    pub fn has_cache(&self, name: &str) -> bool {
        self.caches.read().unwrap().contains_key(name)
    }

    pub fn has_rate_limit(&self, name: &str) -> bool {
        self.rate_limits.read().unwrap().contains_key(name)
    }

    /// Instantiates a processor from `conf`, wrapping it in `ObservedProcessor` so metrics and
    /// `DetailedError` source context (`label`, `type`, `path`) are automatically attached.
    pub fn build_processor(
        &self,
        conf: &ComponentConfig,
        parent_ctx: &ProcContext,
        path_seg: &str,
    ) -> Result<Arc<dyn Processor>, CoreError> {
        let ctor = self
            .processors
            .read()
            .unwrap()
            .get(&conf.component_type)
            .cloned()
            .ok_or_else(|| {
                CoreError::config(format!(
                    "type '{}' was not recognised as a processor",
                    conf.component_type
                ))
            })?;

        let child_ctx =
            parent_ctx.into_child(conf.component_type.clone(), conf.label.clone(), path_seg);
        let inner = ctor(conf, &child_ctx)?;
        Ok(Arc::new(ObservedProcessor::with_identity(
            inner,
            child_ctx,
            &conf.label,
        )))
    }

    pub fn build_processor_list(
        &self,
        confs: &[ComponentConfig],
        parent_ctx: &ProcContext,
    ) -> Result<Vec<Arc<dyn Processor>>, CoreError> {
        let mut out = Vec::with_capacity(confs.len());
        for (i, c) in confs.iter().enumerate() {
            out.push(self.build_processor(c, parent_ctx, &i.to_string())?);
        }
        Ok(out)
    }

    /// Builds an input. A top-level input has the path `root.input`; a child built by a broker,
    /// sequence or read_until constructor gets its path below its parent's.
    pub fn build_input(
        &self,
        conf: &ComponentConfig,
        ctx: &StreamContext,
    ) -> Result<Box<dyn Input>, CoreError> {
        let path = next_child_path("input", "root.input");
        self.build_input_at(conf, ctx, &path)
    }

    /// Builds an input observed under the Bento config path `path` (e.g.
    /// `root.input_resources.<label>`).
    pub fn build_input_at(
        &self,
        conf: &ComponentConfig,
        ctx: &StreamContext,
        path: &str,
    ) -> Result<Box<dyn Input>, CoreError> {
        let ctor = self
            .inputs
            .read()
            .unwrap()
            .get(&conf.component_type)
            .cloned()
            .ok_or_else(|| {
                CoreError::config(format!(
                    "type '{}' was not recognised as an input",
                    conf.component_type
                ))
            })?;
        let mut sctx = ctx.clone();
        if !conf.label.is_empty() {
            sctx.label = conf.label.clone();
        }
        let inner = {
            let _frame = push_frame(path, &conf.component_type);
            ctor(conf, &sctx)?
        };
        let ns = ctx.metrics.component(&conf.label, path);
        let inner: Box<dyn Input> = Box::new(crate::observed::ObservedInput::new(inner, ns));
        if conf.processors.is_empty() {
            return Ok(inner);
        }
        let pctx = sctx.to_proc_context(
            conf.component_type.clone(),
            conf.label.clone(),
            proc_ctx_path(path, "input"),
        );
        let procs = self.build_processor_list(&conf.processors, &pctx)?;
        Ok(Box::new(crate::pipeline::InputWithProcessors::new(
            inner, procs, pctx,
        )))
    }

    /// Builds an output. A top-level output has the path `root.output`; a child built by a
    /// broker, fallback, switch or retry constructor gets its path below its parent's.
    pub fn build_output(
        &self,
        conf: &ComponentConfig,
        ctx: &StreamContext,
    ) -> Result<Arc<dyn Output>, CoreError> {
        let path = next_child_path("output", "root.output");
        self.build_output_at(conf, ctx, &path)
    }

    /// Builds an output observed under the Bento config path `path`.
    pub fn build_output_at(
        &self,
        conf: &ComponentConfig,
        ctx: &StreamContext,
        path: &str,
    ) -> Result<Arc<dyn Output>, CoreError> {
        let ctor = self
            .outputs
            .read()
            .unwrap()
            .get(&conf.component_type)
            .cloned()
            .ok_or_else(|| {
                CoreError::config(format!(
                    "type '{}' was not recognised as an output",
                    conf.component_type
                ))
            })?;
        let mut sctx = ctx.clone();
        if !conf.label.is_empty() {
            sctx.label = conf.label.clone();
        }
        let mut inner = {
            let _frame = push_frame(path, &conf.component_type);
            ctor(conf, &sctx)?
        };
        if let Some(policy) = conf.config.get("batching") {
            let policy = crate::config::BatchPolicyConfig::from_json_value(policy)?;
            if !policy.is_noop() {
                let pctx = sctx.to_proc_context(
                    conf.component_type.clone(),
                    conf.label.clone(),
                    format!(
                        "{}.batching.processors",
                        path.strip_prefix("root.").unwrap_or(path)
                    ),
                );
                let batcher = crate::pipeline::Batcher::new(&policy, pctx)?;
                inner = Arc::new(crate::pipeline::BatchedOutput::new(inner, batcher));
            }
        }
        let ns = ctx.metrics.component(&conf.label, path);
        let inner: Arc<dyn Output> = Arc::new(crate::observed::ObservedOutput::new(inner, ns));
        if conf.processors.is_empty() {
            return Ok(inner);
        }
        let pctx = sctx.to_proc_context(
            conf.component_type.clone(),
            conf.label.clone(),
            proc_ctx_path(path, "output"),
        );
        let procs = self.build_processor_list(&conf.processors, &pctx)?;
        Ok(Arc::new(crate::pipeline::OutputWithProcessors::new(
            inner, procs, pctx,
        )))
    }

    pub fn build_cache(
        &self,
        conf: &ComponentConfig,
        ctx: &StreamContext,
    ) -> Result<Arc<dyn Cache>, CoreError> {
        let ctor = self
            .caches
            .read()
            .unwrap()
            .get(&conf.component_type)
            .cloned()
            .ok_or_else(|| {
                CoreError::config(format!(
                    "type '{}' was not recognised as a cache",
                    conf.component_type
                ))
            })?;
        ctor(conf, ctx)
    }

    pub fn build_rate_limit(
        &self,
        conf: &ComponentConfig,
        ctx: &StreamContext,
    ) -> Result<Arc<dyn RateLimit>, CoreError> {
        let ctor = self
            .rate_limits
            .read()
            .unwrap()
            .get(&conf.component_type)
            .cloned()
            .ok_or_else(|| {
                CoreError::config(format!(
                    "type '{}' was not recognised as a rate_limit",
                    conf.component_type
                ))
            })?;
        ctor(conf, ctx)
    }
}

/// The processor context path below a component at Bento path `path`: the top-level ones keep
/// their historical `input.processors` / `output.processors` (the `DetailedError` path), the
/// others are `<path without root.>.processors`.
fn proc_ctx_path(path: &str, kind: &str) -> String {
    let top = format!("root.{kind}");
    if path == top {
        format!("{kind}.processors")
    } else {
        format!("{}.processors", path.strip_prefix("root.").unwrap_or(path))
    }
}

impl Registry {
    /// Lints a `StreamConfig` for unknown component types, duplicate labels, invalid Bloblang mappings,
    /// `resource:` with a label, and `try` nested directly inside `catch` without clearing errors first.
    pub fn lint_stream_config(&self, conf: &StreamConfig, env_lints: Vec<String>) -> LintReport {
        let mut report = LintReport::default();
        for e in env_lints {
            report.add_error(e);
        }
        let mut seen_labels = HashSet::new();
        let blobl_env = ekiben_bloblang::Environment::new();

        // Check input
        if !self.has_input(&conf.input.component_type) {
            report.add_error(format!(
                "input: type '{}' was not recognised",
                conf.input.component_type
            ));
        }
        check_label(&conf.input, &mut seen_labels, &mut report);
        lint_blobl_fields(&conf.input, "input", &blobl_env, &mut report);
        for p in &conf.input.processors {
            self.lint_processor(p, &blobl_env, &mut seen_labels, &mut report);
        }

        // Check pipeline processors
        for p in &conf.pipeline.processors {
            self.lint_processor(p, &blobl_env, &mut seen_labels, &mut report);
        }

        // Check output
        if !self.has_output(&conf.output.component_type) {
            report.add_error(format!(
                "output: type '{}' was not recognised",
                conf.output.component_type
            ));
        }
        check_label(&conf.output, &mut seen_labels, &mut report);
        lint_blobl_fields(&conf.output, "output", &blobl_env, &mut report);
        for p in &conf.output.processors {
            self.lint_processor(p, &blobl_env, &mut seen_labels, &mut report);
        }

        report
    }

    // ponytail: checks the top-level bloblang fields of inputs and outputs (generate.mapping,
    // http_client.verb is not one); nested ones (switch cases) need Bento's typed field specs.
    fn lint_processor(
        &self,
        proc_conf: &ComponentConfig,
        blobl_env: &ekiben_bloblang::Environment,
        seen_labels: &mut HashSet<String>,
        report: &mut LintReport,
    ) {
        if !self.has_processor(&proc_conf.component_type) {
            report.add_error(format!(
                "processor: type '{}' was not recognised",
                proc_conf.component_type
            ));
            return;
        }
        check_label(proc_conf, seen_labels, report);

        match proc_conf.component_type.as_str() {
            "mapping" | "bloblang" | "mutation" => {
                if let Some(m_str) = proc_conf.config.as_str() {
                    if let Err(e) = blobl_env.compile_mapping(m_str) {
                        report.add_error(format!("processor {}: {e}", proc_conf.component_type));
                    }
                } else {
                    report.add_error(format!(
                        "processor {} requires a string mapping",
                        proc_conf.component_type
                    ));
                }
            }
            "branch" => {
                if let Some(obj) = proc_conf.config.as_object() {
                    for k in ["request_map", "result_map"] {
                        if let Some(s) = obj.get(k).and_then(JsonValue::as_str) {
                            if !s.is_empty() {
                                if let Err(e) = blobl_env.compile_mapping(s) {
                                    report.add_error(format!("branch.{k}: {e}"));
                                }
                            }
                        }
                    }
                    if let Some(arr) = obj.get("processors").and_then(JsonValue::as_array) {
                        for child in arr {
                            if let Ok(c) = ComponentConfig::from_json_value(child, "noop", None) {
                                self.lint_processor(&c, blobl_env, seen_labels, report);
                            }
                        }
                    }
                }
            }
            "catch" => {
                if let Some(arr) = proc_conf.config.as_array() {
                    let mut error_cleared = false;
                    for child in arr {
                        if let Ok(c) = ComponentConfig::from_json_value(child, "noop", None) {
                            if c.component_type == "catch"
                                && c.config.as_array().is_some_and(Vec::is_empty)
                            {
                                error_cleared = true;
                            }
                            if c.component_type == "try" && !error_cleared {
                                report.add_error(
                                    "'catch' block contains a 'try' block which will never execute due to errors only being cleared at the end of the 'catch'",
                                );
                            }
                            self.lint_processor(&c, blobl_env, seen_labels, report);
                        }
                    }
                }
            }
            "try" | "for_each" | "processors" => {
                if let Some(arr) = proc_conf.config.as_array() {
                    for child in arr {
                        if let Ok(c) = ComponentConfig::from_json_value(child, "noop", None) {
                            self.lint_processor(&c, blobl_env, seen_labels, report);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn check_label(conf: &ComponentConfig, seen_labels: &mut HashSet<String>, report: &mut LintReport) {
    if !conf.label.is_empty() {
        if conf.component_type == "resource" {
            report.add_error("a 'resource' component cannot have a 'label' field");
        }
        if !seen_labels.insert(conf.label.clone()) {
            report.add_error(format!(
                "duplicate component label '{}' in stream config",
                conf.label
            ));
        }
    }
}

/// Compiles the bloblang-typed fields Bento lints on inputs and outputs (`mapping`, `check`).
fn lint_blobl_fields(
    conf: &ComponentConfig,
    role: &str,
    blobl_env: &ekiben_bloblang::Environment,
    report: &mut LintReport,
) {
    let Some(obj) = conf.config.as_object() else {
        return;
    };
    for key in ["mapping", "check"] {
        if let Some(m) = obj.get(key).and_then(JsonValue::as_str) {
            if let Err(e) = blobl_env.compile_mapping(m) {
                report.add_error(format!("{role} {}.{key}: {e}", conf.component_type));
            }
        }
    }
}
