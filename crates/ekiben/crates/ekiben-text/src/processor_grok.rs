//! The `grok` processor (`internal/impl/pure/processor_grok.go`, Bento 1.22.0) with the pattern
//! compiler of github.com/Jeffail/grok v1.1.0 (`grokpattern.go`, `patternmap.go`,
//! `compiledgrok.go`): `%{NAME}`, `%{NAME:alias}` and `%{NAME:alias:int|float|string}` expand to Go
//! regular expressions (compiled with Go's grammar by `ekiben_bloblang::go_regex`), the first
//! expression that captures anything wins, and each capture is set at its alias as a dot path.
//!
//! Limits Bento lacks: an expanded expression of at most [`MAX_EXPRESSION_BYTES`] and pattern
//! nesting of at most [`MAX_DEPTH`], so a self-referencing or exploding pattern set is refused at
//! config time instead of overflowing the stack.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use ekiben_bloblang::go_regex::{self, GoRegex};
use ekiben_bloblang::go_strconv;
use ekiben_bloblang::Value;
use ekiben_core::{Batch, ComponentConfig, CoreError, Part, ProcContext, Processor, Registry};
use regex::Regex;

use crate::fields::Fields;
use crate::grok_patterns::DEFAULT_PATTERNS;

pub const MAX_EXPRESSION_BYTES: usize = 1 << 20;
pub const MAX_DEPTH: usize = 64;

static NAMED_REFERENCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"%\{([A-Za-z0-9_\-.]+(?::[A-Za-z0-9_\-.]+(?::[A-Za-z0-9_\-.]+)?)?)\}").expect("static pattern")
});

pub fn register(registry: &Registry) {
    registry.register_processor("grok", |conf, ctx| {
        Ok(Arc::new(GrokProcessor::from_config(conf, ctx.file_root.as_deref())?))
    });
}

/// One expanded pattern (`grokPattern`).
#[derive(Clone)]
struct GrokPattern {
    expression: String,
    safe_aliases: HashMap<String, String>,
    type_hints: HashMap<String, String>,
}

/// One compiled expression (`CompiledGrok`).
pub struct CompiledGrok {
    regexp: GoRegex,
    safe_aliases: HashMap<String, String>,
    type_hints: HashMap<String, String>,
    remove_empty: bool,
}

/// Implements `grok`.
pub struct GrokProcessor {
    parsers: Vec<CompiledGrok>,
}

/// The fields of `grokProcConfig`.
pub struct GrokConfig {
    pub expressions: Vec<String>,
    pub pattern_definitions: BTreeMap<String, String>,
    pub remove_empty: bool,
    pub named_only: bool,
    pub use_defaults: bool,
}

impl GrokProcessor {
    pub fn from_config(conf: &ComponentConfig, root: Option<&Path>) -> Result<Self, CoreError> {
        let f = Fields::new(
            "grok",
            &conf.config,
            &[
                "expressions",
                "pattern_definitions",
                "pattern_paths",
                "named_captures_only",
                "use_default_patterns",
                "remove_empty_values",
            ],
        )?;
        let expressions = f
            .string_list("expressions")?
            .ok_or_else(|| f.missing("expressions"))?;
        let mut defs = f.string_map("pattern_definitions")?.unwrap_or_default();
        for path in f.string_list("pattern_paths")?.unwrap_or_default() {
            add_patterns_from_path(&path, root, &mut defs).map_err(|e| {
                CoreError::config(format!(
                    "failed to parse patterns from path '{path}': {e}"
                ))
            })?;
        }
        let g = GrokConfig {
            expressions,
            pattern_definitions: defs,
            remove_empty: f.bool("remove_empty_values")?.unwrap_or(true),
            named_only: f.bool("named_captures_only")?.unwrap_or(true),
            use_defaults: f.bool("use_default_patterns")?.unwrap_or(true),
        };
        Self::new(&g).map_err(CoreError::config)
    }

    /// Bento's `newGrok`.
    pub fn new(conf: &GrokConfig) -> Result<Self, String> {
        let mut known = HashMap::new();
        if conf.use_defaults {
            let defaults: BTreeMap<String, String> = DEFAULT_PATTERNS
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            add_list(&mut known, &defaults, conf.named_only)
                .map_err(|e| format!("failed to create grok compiler: {e}"))?;
        }
        add_list(&mut known, &conf.pattern_definitions, conf.named_only)
            .map_err(|e| format!("failed to create grok compiler: {e}"))?;
        let mut parsers = Vec::with_capacity(conf.expressions.len());
        for pattern in &conf.expressions {
            let compiled = compile(pattern, &known, conf.named_only, conf.remove_empty)
                .map_err(|e| format!("failed to compile Grok pattern '{pattern}': {e}"))?;
            parsers.push(compiled);
        }
        Ok(Self { parsers })
    }

    /// Bento's `Process`.
    pub fn process(&self, part: &mut Part) -> Result<(), String> {
        let body = part.as_bytes();
        let text = String::from_utf8_lossy(&body);
        let mut values = Vec::new();
        for parser in &self.parsers {
            match parser.parse_typed(&text) {
                Ok(v) => values = v,
                Err(_) => continue,
            }
            if !values.is_empty() {
                break;
            }
        }
        if values.is_empty() {
            return Err("no pattern matches found".to_string());
        }
        let mut obj = Value::Object(BTreeMap::new());
        for (k, v) in values {
            set_path(&mut obj, &k, v);
        }
        part.set_structured(obj);
        Ok(())
    }
}

#[async_trait]
impl Processor for GrokProcessor {
    fn processor_type(&self) -> &'static str {
        "grok"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        let mut out = Batch::new();
        for mut part in batch {
            if let Err(e) = self.process(&mut part) {
                ctx.on_error(&mut part, e);
            }
            out.push(part);
        }
        Ok(vec![out])
    }
}

/// `addGrokPatternsFromPath`: each non-empty line not starting with `#` is `NAME pattern`; a
/// directory reads every file in it. Paths stay under the stream's file root.
fn add_patterns_from_path(
    path: &str,
    root: Option<&Path>,
    patterns: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    if path.contains(['*', '?', '[']) {
        return Err("glob patterns are not supported here; name a file or a directory".into());
    }
    let full = ekiben_io_file::path_guard::sanitize_file_path(path, root)
        .map_err(|e| e.to_string())?;
    let meta = std::fs::metadata(&full).map_err(|e| format!("stat {path}: {e}"))?;
    let mut files = Vec::new();
    if meta.is_dir() {
        for entry in std::fs::read_dir(&full).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.file_type().map_err(|e| e.to_string())?.is_file() {
                files.push(entry.path());
            }
        }
        files.sort();
    } else {
        files.push(full);
    }
    for f in files {
        let text = std::fs::read_to_string(&f).map_err(|e| e.to_string())?;
        for l in text.lines() {
            if l.is_empty() || l.starts_with('#') {
                continue;
            }
            // Go's `names[1]` panics on a line without a space; refuse the file instead.
            let (name, pattern) = l
                .split_once(' ')
                .ok_or_else(|| format!("line '{l}' has no pattern after its name"))?;
            patterns.insert(name.to_string(), pattern.to_string());
        }
    }
    Ok(())
}

/// `patternMap.addList`: a name already known is kept (defaults win over a definition of the same
/// name), every other definition resolves its references first.
fn add_list(
    known: &mut HashMap<String, GrokPattern>,
    new: &BTreeMap<String, String>,
    named_only: bool,
) -> Result<(), String> {
    for (key, pattern) in new {
        if known.contains_key(key) {
            continue;
        }
        resolve(known, key, pattern, new, named_only, 0)?;
    }
    Ok(())
}

/// `patternMap.resolve`.
fn resolve(
    known: &mut HashMap<String, GrokPattern>,
    key: &str,
    pattern: &str,
    new: &BTreeMap<String, String>,
    named_only: bool,
    depth: usize,
) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!(
            "pattern %{{{key}}} nests more than {MAX_DEPTH} references deep"
        ));
    }
    for caps in NAMED_REFERENCE.captures_iter(pattern) {
        let ref_key = caps[1].split(':').next().unwrap_or_default();
        if !known.contains_key(ref_key) {
            let ref_pattern = new
                .get(ref_key)
                .ok_or_else(|| format!("no pattern found for %{{{ref_key}}}"))?;
            // Go drops this error; the `add` below reports the same missing reference.
            let _ = resolve(known, ref_key, ref_pattern, new, named_only, depth + 1);
        }
    }
    let p = new_pattern(pattern, known, named_only)?;
    known.insert(key.to_string(), p);
    Ok(())
}

/// `newPattern`: every reference replaced by its expression, as a named group when it has an
/// alias (or always, without `named_captures_only`).
fn new_pattern(
    pattern: &str,
    known: &HashMap<String, GrokPattern>,
    named_only: bool,
) -> Result<GrokPattern, String> {
    let mut type_hints = HashMap::new();
    let mut safe_aliases = HashMap::new();
    let mut out = pattern.to_string();
    for caps in NAMED_REFERENCE.captures_iter(pattern) {
        let whole = &caps[0];
        let names: Vec<&str> = caps[1].split(':').collect();
        let ref_key = names[0];
        let mut ref_alias = names.get(1).copied().unwrap_or(ref_key).to_string();
        let safe: String = ref_alias
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
            .collect();
        if safe != ref_alias {
            safe_aliases.insert(safe.clone(), ref_alias.clone());
            ref_alias = safe;
        }
        if names.len() == 3 && names[2] != "string" {
            type_hints.insert(ref_alias.clone(), names[2].to_string());
        }
        let ref_pattern = known
            .get(ref_key)
            .ok_or_else(|| format!("no pattern found for %{{{ref_key}}}"))?;
        let expr = if !named_only || names.len() > 1 {
            format!("(?P<{ref_alias}>{})", ref_pattern.expression)
        } else {
            format!("({})", ref_pattern.expression)
        };
        for (k, t) in &ref_pattern.type_hints {
            type_hints.entry(k.clone()).or_insert_with(|| t.to_lowercase());
        }
        for (s, r) in &ref_pattern.safe_aliases {
            safe_aliases.entry(s.clone()).or_insert_with(|| r.clone());
        }
        out = out.replace(whole, &expr);
        if out.len() > MAX_EXPRESSION_BYTES {
            return Err(format!(
                "the expanded expression is over {MAX_EXPRESSION_BYTES} bytes"
            ));
        }
    }
    Ok(GrokPattern {
        expression: out,
        safe_aliases,
        type_hints,
    })
}

/// `Grok.Compile`.
fn compile(
    pattern: &str,
    known: &HashMap<String, GrokPattern>,
    named_only: bool,
    remove_empty: bool,
) -> Result<CompiledGrok, String> {
    let p = new_pattern(pattern, known, named_only)?;
    let regexp = go_regex::compile(&p.expression)?;
    Ok(CompiledGrok {
        regexp,
        safe_aliases: p.safe_aliases,
        type_hints: p.type_hints,
        remove_empty,
    })
}

impl CompiledGrok {
    /// `ParseTyped`: the captures in group order (a later group of the same name wins, as in
    /// Go's map), typed by their hints.
    pub fn parse_typed(&self, text: &str) -> Result<Vec<(String, Value)>, String> {
        let mut out: Vec<(String, Value)> = Vec::new();
        let Some(caps) = self.regexp.captures(text) else {
            return Ok(out);
        };
        for (idx, key) in self.regexp.group_names().enumerate() {
            let key = key.unwrap_or_default();
            let m = caps.get(idx).map_or("", |m| m.as_str());
            if key.is_empty() || (self.remove_empty && m.is_empty()) {
                continue;
            }
            let v = self.type_cast(m, key)?;
            let name = self.safe_aliases.get(key).map_or(key, String::as_str);
            match out.iter_mut().find(|(k, _)| k == name) {
                Some(slot) => slot.1 = v,
                None => out.push((name.to_string(), v)),
            }
        }
        Ok(out)
    }

    fn type_cast(&self, m: &str, key: &str) -> Result<Value, String> {
        match self.type_hints.get(key).map(String::as_str) {
            None | Some("string") => Ok(Value::String(m.to_string())),
            Some("int") => go_strconv::atoi(m).map(Value::Int),
            Some("float") => go_strconv::parse_float(m, 64).map(Value::Float),
            Some(t) => Err(format!(
                "ERROR the value {m} cannot be converted to {t}. Must be int, float, string or empty"
            )),
        }
    }
}

/// gabs `SetP`: the dot path's objects created on the way; a path through a non-object is
/// skipped, as gabs fails it and Bento ignores the error.
fn set_path(root: &mut Value, path: &str, v: Value) {
    let mut cur = root;
    let segs: Vec<&str> = path.split('.').collect();
    for (i, seg) in segs.iter().enumerate() {
        let Value::Object(m) = cur else { return };
        if i + 1 == segs.len() {
            m.insert((*seg).to_string(), v);
            return;
        }
        cur = m
            .entry((*seg).to_string())
            .or_insert_with(|| Value::Object(BTreeMap::new()));
    }
}
