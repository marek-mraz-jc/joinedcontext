//! `ekiben test`: Bento's config unit tests (`internal/cli/test`, `internal/config/test`).
//!
//! A target config (or its `<name>_bento_test.yaml` sibling) holds a `tests:` list; each case feeds
//! input batches through the targeted processors (or a Bloblang file) and checks the output batches.
//! The report on stdout follows `bento test` line for line.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ekiben_bloblang::{Environment, Importer, Value};
use ekiben_core::{
    execute_all, interpolate_env_str, Batch, ComponentConfig, EnvLintError, OrgId, Part, PartsView,
    ProcContext, Registry, ResourceManager, StreamConfig, StreamContext, StreamMetrics,
};
use serde_json::Value as Json;
use tokio::sync::watch;

use crate::runner::register_inline_resources;

/// Bento's definition file suffix (`bento test` passes `_bento_test`).
pub const TEST_SUFFIX: &str = "_bento_test";

/// One parsed test case (`internal/config/test/case.go:Case`).
#[derive(Debug, Clone)]
pub struct Case {
    pub name: String,
    pub line: usize,
    pub environment: HashMap<String, String>,
    pub target_processors: String,
    pub target_mapping: String,
    pub mocks: Vec<(String, Json)>,
    pub input_batches: Vec<Vec<InputMsg>>,
    pub output_batches: Vec<Vec<BTreeMap<String, Condition>>>,
}

#[derive(Debug, Clone, Default)]
pub struct InputMsg {
    pub content: String,
    pub file: Option<String>,
    pub metadata: Vec<(String, Json)>,
}

/// One output condition (`internal/config/test/output.go`).
#[derive(Debug, Clone)]
pub enum Condition {
    Bloblang(String),
    ContentEquals(String),
    ContentMatches(String),
    MetadataEquals(Vec<(String, Json)>),
    FileEquals(String),
    FileJsonEquals(String),
    FileJsonContains(String),
    JsonEquals(String),
    JsonContains(String),
}

/// Runs every test definition found under `paths` and writes Bento's report to `out`
/// (`internal/cli/test/command.go:RunAll`). Returns `true` when every target passed.
pub async fn run_all(
    registry: Arc<Registry>,
    paths: &[PathBuf],
    resources_paths: &[PathBuf],
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> bool {
    let targets = match get_test_targets(paths) {
        Ok(t) => t,
        Err(e) => {
            let _ = writeln!(err, "Failed to obtain test targets: {e}");
            return false;
        }
    };
    if targets.is_empty() {
        let _ = writeln!(out, "No tests were found");
        return false;
    }

    let mut fails: Vec<(String, Vec<CaseFailure>)> = Vec::new();
    for (target, cases) in &targets {
        match execute(&registry, cases, target, resources_paths).await {
            Ok(failures) if failures.is_empty() => {
                let _ = writeln!(out, "Test '{target}' succeeded");
            }
            Ok(failures) => {
                let _ = writeln!(out, "Test '{target}' failed");
                fails.push((target.clone(), failures));
            }
            Err(e) => {
                let _ = writeln!(err, "Failed to execute test target '{target}': {e}");
                return false;
            }
        }
    }
    if fails.is_empty() {
        return true;
    }
    let _ = write!(out, "\nFailures:\n\n");
    for (i, (target, cases)) in fails.iter().enumerate() {
        if i > 0 {
            let _ = writeln!(out);
        }
        let _ = write!(out, "--- {target} ---\n\n");
        let mut name_prev: Option<&str> = None;
        for (j, f) in cases.iter().enumerate() {
            if name_prev != Some(f.name.as_str()) {
                if j > 0 {
                    let _ = writeln!(out);
                }
                let _ = writeln!(out, "{} [line {}]:", f.name, f.line);
                name_prev = Some(f.name.as_str());
            }
            let _ = writeln!(out, "{}", f.reason);
        }
    }
    false
}

#[derive(Debug, Clone, PartialEq)]
pub struct CaseFailure {
    pub name: String,
    pub line: usize,
    pub reason: String,
}

/// Go's `filepath.Clean` for the relative and absolute paths `bento test` prints.
fn clean(p: &str) -> String {
    let abs = p.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." if parts.last().is_some_and(|l| *l != "..") => {
                parts.pop();
            }
            ".." if abs => {}
            s => parts.push(s),
        }
    }
    let joined = parts.join("/");
    match (abs, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".to_string(),
        (false, false) => joined,
    }
}

fn is_yaml(p: &str) -> bool {
    p.ends_with(".yaml") || p.ends_with(".yml")
}

/// Expands `dir/...` (recursive), a directory (its own files) or a file into YAML paths
/// (`internal/filepath.GlobsAndSuperPaths`). Shell globs arrive expanded.
fn expand_paths(paths: &[PathBuf]) -> Result<Vec<String>, String> {
    fn walk(dir: &str, recursive: bool, acc: &mut Vec<String>) -> Result<(), String> {
        let rd = std::fs::read_dir(dir).map_err(|e| format!("{dir}: {e}"))?;
        let mut entries: Vec<_> = rd.filter_map(Result::ok).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = clean(&format!("{dir}/{}", e.file_name().to_string_lossy()));
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                if recursive {
                    walk(&p, true, acc)?;
                }
            } else if is_yaml(&p) {
                acc.push(p);
            }
        }
        Ok(())
    }
    let mut acc = Vec::new();
    for raw in paths {
        let s = raw.to_string_lossy();
        if let Some(root) = s.strip_suffix("...") {
            walk(&clean(root), true, &mut acc)?;
        } else if Path::new(s.as_ref()).is_dir() {
            walk(&clean(&s), false, &mut acc)?;
        } else {
            acc.push(clean(&s));
        }
    }
    Ok(acc)
}

/// Splits a path into (config path, definition path) (`GetPathPair`).
fn path_pair(full: &str) -> (String, String) {
    let (dir, file) = match full.rfind('/') {
        Some(i) => (&full[..=i], &full[i + 1..]),
        None => ("", full),
    };
    let (stem, ext) = match file.rfind('.') {
        Some(i) => (&file[..i], &file[i..]),
        None => (file, ""),
    };
    match stem.strip_suffix(TEST_SUFFIX) {
        Some(base) => (clean(&format!("{dir}{base}{ext}")), clean(full)),
        None => (
            clean(full),
            clean(&format!("{dir}{stem}{TEST_SUFFIX}{ext}")),
        ),
    }
}

/// Finds every target config with test cases, sorted by path (`GetTestTargets`).
pub fn get_test_targets(paths: &[PathBuf]) -> Result<BTreeMap<String, Vec<Case>>, String> {
    let mut out = BTreeMap::new();
    for p in expand_paths(paths)? {
        let (config_path, mut def_path) = path_pair(&p);
        if let Err(e) = std::fs::metadata(&config_path) {
            return Err(format!(
                "unable to access target config file '{config_path}': {e}"
            ));
        }
        if std::fs::metadata(&def_path).is_err() {
            if !is_yaml(&config_path) {
                continue;
            }
            def_path = config_path.clone();
        }
        let text = std::fs::read_to_string(&def_path)
            .map_err(|e| format!("failed to read test definition from '{def_path}': {e}"))?;
        let cases = parse_definition(&text)
            .map_err(|e| format!("failed to parse test definition from '{def_path}': {e}"))?;
        if !cases.is_empty() {
            out.insert(config_path, cases);
        }
    }
    Ok(out)
}

/// Line of each top-level item of the `tests:` sequence, as yaml.v3 reports a mapping node's line.
/// ponytail: block-style sequences only (every Bento suite uses them); flow-style lists get line 0.
fn case_lines(text: &str) -> Vec<usize> {
    let lines: Vec<&str> = text.lines().collect();
    let Some(start) = lines.iter().position(|l| l.starts_with("tests:")) else {
        return Vec::new();
    };
    let mut item_indent: Option<usize> = None;
    let mut out = Vec::new();
    let mut pending_dash = false;
    for (i, l) in lines.iter().enumerate().skip(start + 1) {
        let trimmed = l.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = l.len() - trimmed.len();
        if indent == 0 && !trimmed.starts_with('-') {
            break;
        }
        if pending_dash {
            out.push(i + 1);
            pending_dash = false;
            continue;
        }
        if trimmed.starts_with('-') && item_indent.is_none_or(|ii| ii == indent) {
            item_indent = Some(indent);
            if trimmed.trim_end() == "-" {
                pending_dash = true;
            } else {
                out.push(i + 1);
            }
        }
    }
    out
}

/// Go's `fmt.Sprint` of a YAML scalar read as a string field.
fn scalar_string(v: &Json) -> Result<String, String> {
    match v {
        Json::String(s) => Ok(s.clone()),
        Json::Number(n) => Ok(n.to_string()),
        Json::Bool(b) => Ok(b.to_string()),
        Json::Null => Ok(String::new()),
        other => Err(format!("expected string value, got {}", type_name(other))),
    }
}

fn type_name(v: &Json) -> &'static str {
    match v {
        Json::Null => "null",
        Json::Bool(_) => "bool",
        Json::Number(_) => "number",
        Json::String(_) => "string",
        Json::Array(_) => "array",
        Json::Object(_) => "object",
    }
}

/// Parses a test definition document (`internal/config/test.FromAny`).
pub fn parse_definition(text: &str) -> Result<Vec<Case>, String> {
    let root: Json = serde_norway::from_str(text).map_err(|e| e.to_string())?;
    let Some(tests) = root.get("tests") else {
        return Ok(Vec::new());
    };
    let Some(list) = tests.as_array() else {
        return Err("field tests: expected array value".to_string());
    };
    let lines = case_lines(text);
    list.iter()
        .enumerate()
        .map(|(i, c)| parse_case(c, lines.get(i).copied().unwrap_or(0)))
        .collect()
}

fn obj_pairs(v: &Json, field: &str) -> Result<Vec<(String, Json)>, String> {
    match v {
        Json::Object(m) => Ok(m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
        Json::Null => Ok(Vec::new()),
        _ => Err(format!("field {field}: expected object value")),
    }
}

fn parse_case(v: &Json, line: usize) -> Result<Case, String> {
    let get_str = |k: &str, default: &str| -> Result<String, String> {
        v.get(k)
            .map(scalar_string)
            .transpose()
            .map(|s| s.unwrap_or_else(|| default.to_string()))
            .map_err(|e| format!("field {k}: {e}"))
    };
    let mut c = Case {
        name: get_str("name", "")?,
        line,
        environment: HashMap::new(),
        target_processors: get_str("target_processors", "/pipeline/processors")?,
        target_mapping: get_str("target_mapping", "")?,
        mocks: Vec::new(),
        input_batches: Vec::new(),
        output_batches: Vec::new(),
    };
    if let Some(env) = v.get("environment") {
        for (k, val) in obj_pairs(env, "environment")? {
            c.environment.insert(k, scalar_string(&val)?);
        }
    }
    if let Some(m) = v.get("mocks") {
        c.mocks = obj_pairs(m, "mocks")?;
    }
    if let Some(Json::Array(batches)) = v.get("input_batches") {
        for b in batches {
            let list = b
                .as_array()
                .ok_or("field input_batches: expected array value")?;
            c.input_batches
                .push(list.iter().map(parse_input).collect::<Result<_, _>>()?);
        }
    }
    if let Some(Json::Array(list)) = v.get("input_batch") {
        if !list.is_empty() {
            c.input_batches
                .push(list.iter().map(parse_input).collect::<Result<_, _>>()?);
        }
    }
    if let Some(Json::Array(batches)) = v.get("output_batches") {
        for b in batches {
            let list = b
                .as_array()
                .ok_or("field output_batches: expected array value")?;
            c.output_batches.push(
                list.iter()
                    .map(parse_conditions)
                    .collect::<Result<_, _>>()?,
            );
        }
    }
    Ok(c)
}

fn parse_input(v: &Json) -> Result<InputMsg, String> {
    let mut m = InputMsg::default();
    if let Some(s) = v.get("content") {
        m.content = scalar_string(s)?;
    }
    if let Some(j) = v.get("json_content") {
        m.content = go_json_marshal(j);
    }
    if let Some(f) = v.get("file_content") {
        m.file = Some(scalar_string(f)?);
    }
    if let Some(meta) = v.get("metadata") {
        m.metadata = obj_pairs(meta, "metadata")?;
    }
    Ok(m)
}

fn parse_conditions(v: &Json) -> Result<BTreeMap<String, Condition>, String> {
    let mut m = BTreeMap::new();
    let Some(obj) = v.as_object() else {
        return Ok(m);
    };
    for (k, val) in obj {
        let s = || scalar_string(val).map_err(|e| format!("field {k}: {e}"));
        let cond = match k.as_str() {
            "bloblang" => {
                let src = s()?;
                Environment::new()
                    .compile_mapping(&src)
                    .map_err(|e| format!("bloblang: {e}"))?;
                Condition::Bloblang(src)
            }
            "content_equals" => Condition::ContentEquals(s()?),
            "content_matches" => Condition::ContentMatches(s()?),
            "metadata_equals" => Condition::MetadataEquals(obj_pairs(val, k)?),
            "file_equals" => Condition::FileEquals(s()?),
            "file_json_equals" => Condition::FileJsonEquals(s()?),
            "file_json_contains" => Condition::FileJsonContains(s()?),
            "json_equals" => Condition::JsonEquals(json_test_string(val)),
            "json_contains" => Condition::JsonContains(json_test_string(val)),
            _ => continue,
        };
        m.insert(k.clone(), cond);
    }
    Ok(m)
}

fn json_test_string(v: &Json) -> String {
    match v {
        Json::String(s) => s.clone(),
        other => go_json_marshal(other),
    }
}

/// Go's `json.Marshal` of a decoded YAML value: sorted keys, HTML-escaped strings.
fn go_json_marshal(v: &Json) -> String {
    fn sort(v: &Json) -> Json {
        match v {
            Json::Object(m) => {
                let mut keys: Vec<_> = m.keys().collect();
                keys.sort();
                Json::Object(keys.into_iter().map(|k| (k.clone(), sort(&m[k]))).collect())
            }
            Json::Array(a) => Json::Array(a.iter().map(sort).collect()),
            other => other.clone(),
        }
    }
    sort(v)
        .to_string()
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

/// Reads `import`/`from` files relative to the mapping file (`WithImporterRelativeToFile`).
struct DirImporter(PathBuf);

impl Importer for DirImporter {
    fn import(&self, path: &str) -> Result<Vec<u8>, String> {
        std::fs::read(self.0.join(path)).map_err(|e| format!("{path}: {e}"))
    }
}

fn dir_of(path: &str) -> String {
    match path.rfind('/') {
        Some(0) => "/".to_string(),
        Some(i) => path[..i].to_string(),
        None => ".".to_string(),
    }
}

fn join(dir: &str, rel: &str) -> String {
    if rel.starts_with('/') {
        clean(rel)
    } else {
        clean(&format!("{dir}/{rel}"))
    }
}

/// Runs every case of one target (`internal/cli/test/definition.go:Execute`).
pub async fn execute(
    registry: &Arc<Registry>,
    cases: &[Case],
    target: &str,
    resources_paths: &[PathBuf],
) -> Result<Vec<CaseFailure>, String> {
    let dir = dir_of(target);
    let mut all = Vec::new();
    for (i, c) in cases.iter().enumerate() {
        let failures = execute_case(registry, c, target, &dir, resources_paths)
            .await
            .map_err(|e| format!("test case {i} failed: {e}"))?;
        all.extend(failures);
    }
    Ok(all)
}

fn lookup_for(env: &HashMap<String, String>) -> impl FnMut(&str) -> Option<String> + '_ {
    move |k| env.get(k).cloned().or_else(|| std::env::var(k).ok())
}

fn read_config(path: &str, env: &HashMap<String, String>) -> Result<Json, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to parse config file '{path}': {e}"))?;
    let swapped = match interpolate_env_str(&raw, lookup_for(env)) {
        Ok(s) => s,
        Err(EnvLintError { best_attempt, .. }) => best_attempt,
    };
    serde_norway::from_str(&swapped)
        .map_err(|e| format!("failed to parse config file '{path}': {e}"))
}

/// JSON Pointer (RFC 6901) to path segments.
fn pointer_segments(ptr: &str) -> Vec<String> {
    ptr.split('/')
        .skip(1)
        .map(|s| s.replace("~1", "/").replace("~0", "~"))
        .collect()
}

fn get_path<'a>(root: &'a Json, segs: &[String]) -> Result<&'a Json, String> {
    let mut cur = root;
    for s in segs {
        cur = match cur {
            Json::Object(m) => m.get(s),
            Json::Array(a) => s.parse::<usize>().ok().and_then(|i| a.get(i)),
            _ => None,
        }
        .ok_or_else(|| format!("path '{s}' not found"))?;
    }
    Ok(cur)
}

/// Sets `val` at `segs`, `-` appending to an array (`docs.FieldSpecs.SetYAMLPath`).
fn set_path(root: &mut Json, segs: &[String], val: Json) -> Result<(), String> {
    let Some((last, parents)) = segs.split_last() else {
        *root = val;
        return Ok(());
    };
    let mut cur = root;
    for s in parents {
        cur = match cur {
            Json::Object(m) => m
                .entry(s.clone())
                .or_insert_with(|| Json::Object(Default::default())),
            Json::Array(a) => s
                .parse::<usize>()
                .ok()
                .and_then(|i| a.get_mut(i))
                .ok_or_else(|| format!("index '{s}' out of bounds"))?,
            _ => return Err(format!("path '{s}' is not a container")),
        };
    }
    match cur {
        Json::Object(m) => {
            m.insert(last.clone(), val);
        }
        Json::Array(a) if last == "-" => a.push(val),
        Json::Array(a) => {
            let slot = last
                .parse::<usize>()
                .ok()
                .and_then(|i| a.get_mut(i))
                .ok_or_else(|| format!("index '{last}' out of bounds"))?;
            *slot = val;
        }
        _ => return Err(format!("path '{last}' is not a container")),
    }
    Ok(())
}

/// Collects `label` → path of every labelled component outside `tests:` (`YAMLLabelsToPaths`).
fn labels_to_paths(root: &Json) -> HashMap<String, Vec<String>> {
    fn walk(v: &Json, path: &mut Vec<String>, acc: &mut HashMap<String, Vec<String>>) {
        match v {
            Json::Object(m) => {
                if let Some(Json::String(l)) = m.get("label") {
                    if !l.is_empty() {
                        acc.entry(l.clone()).or_insert_with(|| path.clone());
                    }
                }
                for (k, child) in m {
                    if path.is_empty() && k == "tests" {
                        continue;
                    }
                    path.push(k.clone());
                    walk(child, path, acc);
                    path.pop();
                }
            }
            Json::Array(a) => {
                for (i, child) in a.iter().enumerate() {
                    path.push(i.to_string());
                    walk(child, path, acc);
                    path.pop();
                }
            }
            _ => {}
        }
    }
    let mut acc = HashMap::new();
    walk(root, &mut Vec::new(), &mut acc);
    acc
}

/// Replaces the component at `segs` with `mock`, keeping the original label (`setMock`).
fn set_mock(root: &mut Json, mock: &Json, segs: &[String]) -> Result<(), String> {
    let mut mock = mock.clone();
    if let Json::Object(m) = &mut mock {
        if m.contains_key("label") {
            m.remove("label");
        } else if let Ok(Json::Object(orig)) = get_path(root, segs) {
            if let Some(l) = orig.get("label") {
                m.insert("label".to_string(), l.clone());
            }
        }
    }
    set_path(root, segs, mock)
}

/// Builds the processors a case targets (`ProcessorsProvider.Provide` / `ProvideBloblang`).
fn build_case_processors(
    registry: &Arc<Registry>,
    c: &Case,
    target: &str,
    dir: &str,
    resources_paths: &[PathBuf],
    pctx: &mut ProcContext,
    sctx: &StreamContext,
) -> Result<Vec<Arc<dyn ekiben_core::Processor>>, String> {
    if !c.target_mapping.is_empty() {
        let path = join(dir, &c.target_mapping);
        let wrap = |e: String| {
            format!(
                "failed to initialise Bloblang mapping '{}': {e}",
                c.target_mapping
            )
        };
        let src = std::fs::read_to_string(&path).map_err(|e| wrap(e.to_string()))?;
        pctx.blobl_env = pctx
            .blobl_env
            .clone()
            .with_importer(Arc::new(DirImporter(PathBuf::from(dir_of(&path)))));
        let conf = ComponentConfig::from_json_value(
            &serde_json::json!({ "mapping": src }),
            "mapping",
            None,
        )
        .map_err(|e| wrap(e.to_string()))?;
        return registry
            .build_processor_list(&[conf], pctx)
            .map_err(|e| wrap(e.to_string()));
    }

    let ptr = &c.target_processors;
    let wrap = |e: String| format!("failed to initialise processors '{ptr}': {e}");
    let (file_part, proc_path) = match ptr.split_once('#') {
        Some((f, frag)) => (Some(f), frag),
        None => (None, ptr.as_str()),
    };
    if proc_path.is_empty() {
        return Err(wrap(format!(
            "failed to target processors '{ptr}': reference URI must contain a path or fragment"
        )));
    }
    let target_path = match file_part {
        Some(f) if !f.is_empty() => join(dir, f),
        _ => target.to_string(),
    };
    let mut root = read_config(&target_path, &c.environment).map_err(wrap)?;

    let mut label_mocks = Vec::new();
    for (k, v) in &c.mocks {
        if k.starts_with('/') {
            set_mock(&mut root, v, &pointer_segments(k))
                .map_err(|e| wrap(format!("failed to set mock '{k}': {e}")))?;
        } else {
            label_mocks.push((k, v));
        }
    }
    if !label_mocks.is_empty() {
        let labels = labels_to_paths(&root);
        for (k, v) in label_mocks {
            let segs = labels.get(k).ok_or_else(|| wrap(format!(
                "mock for label '{k}' could not be applied as the label was not found in the test target file, it is not currently possible to mock resources imported separate to the test file"
            )))?;
            set_mock(&mut root, v, segs)
                .map_err(|e| wrap(format!("failed to set mock '{k}': {e}")))?;
        }
    }

    let parse_err = |e: String| wrap(format!("failed to parse config file '{target_path}': {e}"));
    let mut conf = StreamConfig::from_json_value(&root).map_err(|e| parse_err(e.to_string()))?;
    for p in resources_paths {
        let p = p.to_string_lossy();
        let extra = read_config(&p, &c.environment).map_err(wrap)?;
        let extra = StreamConfig::from_json_value(&extra)
            .map_err(|e| wrap(format!("failed to parse resources config file '{p}': {e}")))?;
        let r = &mut conf.resources;
        r.cache_resources.extend(extra.resources.cache_resources);
        r.rate_limit_resources
            .extend(extra.resources.rate_limit_resources);
        r.processor_resources
            .extend(extra.resources.processor_resources);
    }
    conf.resources.input_resources.clear();
    conf.resources.output_resources.clear();
    conf.org = sctx.org.org_id.clone();
    register_inline_resources(&conf, registry, sctx)
        .map_err(|e| wrap(format!("failed to initialise resources: {e}")))?;

    let segs = if proc_path.starts_with('/') {
        pointer_segments(proc_path)
    } else {
        labels_to_paths(&root).remove(proc_path).ok_or_else(|| wrap(format!(
            "target for label '{proc_path}' failed as the label was not found in the test target file, it is not currently possible to target resources imported separate to the test file"
        )))?
    };
    let node = get_path(&root, &segs).map_err(|e| {
        wrap(format!(
            "failed to resolve case processors from '{target_path}': {e}"
        ))
    })?;
    let nodes: Vec<&Json> = match node {
        Json::Array(a) => a.iter().collect(),
        other => vec![other],
    };
    let confs = nodes
        .into_iter()
        .map(|n| ComponentConfig::from_json_value(n, "noop", None))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| {
            wrap(format!(
                "failed to resolve case processors from '{target_path}': {e}"
            ))
        })?;
    registry
        .build_processor_list(&confs, pctx)
        .map_err(|e| wrap(e.to_string()))
}

/// Runs one case (`internal/cli/test/case.go:ExecuteFrom`).
async fn execute_case(
    registry: &Arc<Registry>,
    c: &Case,
    target: &str,
    dir: &str,
    resources_paths: &[PathBuf],
) -> Result<Vec<CaseFailure>, String> {
    let resources = ResourceManager::new();
    let org_id = OrgId::default();
    let org = resources.get_or_create_org(&org_id);
    let metrics = StreamMetrics::new("test", org_id.0.clone());
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
    let blobl_env = Environment::new()
        .with_host_env(true)
        .with_env_vars(c.environment.clone());
    let sctx = StreamContext {
        stream_id: "test".to_string(),
        org: org.clone(),
        resources: resources.clone(),
        registry: registry.clone(),
        blobl_env: blobl_env.clone(),
        metrics: metrics.clone(),
        label: String::new(),
        path: "resources".to_string(),
        file_root: None,
        env_vars: Arc::new(c.environment.clone()),
        shutdown_rx: shutdown_rx.clone(),
    };
    let mut pctx = ProcContext {
        stream_id: "test".to_string(),
        org,
        resources,
        registry: registry.clone(),
        blobl_env,
        metrics,
        label: String::new(),
        component_type: "test".to_string(),
        path: "processors".to_string(),
        file_root: None,
        shutdown_rx,
    };
    let procs = build_case_processors(registry, c, target, dir, resources_paths, &mut pctx, &sctx)?;

    let mut inputs: Vec<Batch> = Vec::new();
    for batch in &c.input_batches {
        let mut parts = Vec::with_capacity(batch.len());
        for (i, m) in batch.iter().enumerate() {
            let content = match &m.file {
                Some(f) => std::fs::read(join(dir, f))
                    .map_err(|e| format!("failed to create test input {i}: {e}"))?,
                None => m.content.clone().into_bytes(),
            };
            let mut part = Part::new(content);
            for (k, v) in &m.metadata {
                part.meta_set(k.clone(), Value::from_json_value(v.clone()));
            }
            parts.push(part);
        }
        inputs.push(parts);
    }

    let mut failures = Vec::new();
    let mut report = |reason: String| {
        failures.push(CaseFailure {
            name: c.name.clone(),
            line: c.line,
            reason,
        })
    };

    let outputs = match execute_all(&pctx, &procs, inputs).await {
        Ok(o) => o,
        Err(e) => {
            report(format!("processors resulted in error: {e}"));
            Vec::new()
        }
    };
    if outputs.len() < c.output_batches.len() {
        report(format!(
            "wrong batch count, expected {}, got {}",
            c.output_batches.len(),
            outputs.len()
        ));
    }
    for (i, batch) in outputs.iter().enumerate() {
        let Some(expected) = c.output_batches.get(i) else {
            let all: Vec<String> = batch
                .iter()
                .map(|p| String::from_utf8_lossy(&p.as_bytes_ref()).into_owned())
                .collect();
            report(format!("unexpected batch: [{}]", all.join(" ")));
            continue;
        };
        if expected.len() != batch.len() {
            report(format!(
                "mismatch of output batch {i} message counts, expected {}, got {}",
                expected.len(),
                batch.len()
            ));
        }
        for (j, part) in batch.iter().enumerate() {
            let Some(conds) = expected.get(j) else {
                report(format!(
                    "unexpected message from batch {i}: {}",
                    String::from_utf8_lossy(&part.as_bytes_ref())
                ));
                continue;
            };
            let errs = check_all(conds, dir, part, &c.environment);
            for e in &errs {
                report(format!("batch {i} message {j}: {e}"));
            }
            if let (Some(pe), false) = (part.error_get(), errs.is_empty()) {
                report(format!("batch {i} message {j}: {pe}"));
            }
        }
    }
    Ok(failures)
}

fn check_all(
    conds: &BTreeMap<String, Condition>,
    dir: &str,
    part: &Part,
    env: &HashMap<String, String>,
) -> Vec<String> {
    conds
        .iter()
        .filter_map(|(k, c)| check(c, dir, part, env).err().map(|e| format!("{k}: {e}")))
        .collect()
}

fn check(
    c: &Condition,
    dir: &str,
    part: &Part,
    env: &HashMap<String, String>,
) -> Result<(), String> {
    let act = part.as_bytes_ref();
    let act_str = String::from_utf8_lossy(&act);
    let content_eq = |exp: &str| {
        if exp == act_str {
            Ok(())
        } else {
            Err(format!(
                "content mismatch\n  expected: {exp}\n  received: {act_str}"
            ))
        }
    };
    let read_file = |f: &str, what: &str| {
        std::fs::read_to_string(join(dir, f))
            .map_err(|e| format!("failed to read comparison {what}file: {e}"))
    };
    match c {
        Condition::Bloblang(src) => {
            let exec = Environment::new()
                .with_host_env(true)
                .with_env_vars(env.clone())
                .compile_mapping(src)
                .map_err(|e| e.to_string())?;
            let parts = [part.clone()];
            match exec.query_part(0, &PartsView(&parts)) {
                Ok(true) => Ok(()),
                Ok(false) => Err("bloblang expression was false".to_string()),
                Err(e) => Err(e.to_string()),
            }
        }
        Condition::ContentEquals(exp) => content_eq(exp),
        Condition::ContentMatches(pat) => {
            let re = regex::bytes::Regex::new(pat).map_err(|e| format!("invalid pattern: {e}"))?;
            if re.is_match(&act) {
                Ok(())
            } else {
                Err(format!(
                    "pattern mismatch\n   pattern: {pat}\n  received: {act_str}"
                ))
            }
        }
        Condition::MetadataEquals(pairs) => {
            for (k, exp) in pairs {
                let Some(got) = part.meta_get(k) else {
                    return Err(format!("metadata key '{k}' expected but not found"));
                };
                let exp_v = Value::from_json_value(exp.clone());
                if !i_compare(&exp_v, got) {
                    return Err(format!(
                        "metadata key '{k}' mismatch\n  expected: {}\n  received: {}",
                        go_sprint(&exp_v),
                        go_sprint(got)
                    ));
                }
            }
            Ok(())
        }
        Condition::FileEquals(f) => content_eq(&read_file(f, "")?),
        Condition::FileJsonEquals(f) => json_check(&act, &read_file(f, "JSON ")?, false),
        Condition::FileJsonContains(f) => json_check(&act, &read_file(f, "JSON ")?, true),
        Condition::JsonEquals(exp) => json_check(&act, exp, false),
        Condition::JsonContains(exp) => json_check(&act, exp, true),
    }
}

/// `value.ICompare`: expected value against a metadata value.
fn i_compare(exp: &Value, act: &Value) -> bool {
    match exp {
        Value::Null => matches!(act, Value::Null),
        Value::String(s) => match act {
            Value::String(a) => a == s,
            Value::Bytes(b) => b.as_slice() == s.as_bytes(),
            _ => false,
        },
        Value::Int(_) | Value::Uint(_) | Value::Float(_) => {
            matches!(act, Value::Int(_) | Value::Uint(_) | Value::Float(_))
                && exp.as_f64() == act.as_f64()
        }
        Value::Bool(b) => matches!(act, Value::Bool(a) if a == b),
        Value::Array(e) => match act {
            Value::Array(a) => e.len() == a.len() && e.iter().zip(a).all(|(x, y)| i_compare(x, y)),
            _ => false,
        },
        Value::Object(e) => match act {
            Value::Object(a) => {
                e.len() == a.len()
                    && e.iter()
                        .all(|(k, x)| a.get(k).is_some_and(|y| i_compare(x, y)))
            }
            _ => false,
        },
        other => other.compare_eq(act),
    }
}

/// Go's `%v` of a decoded value.
fn go_sprint(v: &Value) -> String {
    match v {
        Value::Null => "<nil>".to_string(),
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(go_sprint).collect::<Vec<_>>().join(" ")
        ),
        Value::Object(m) => format!(
            "map[{}]",
            m.iter()
                .map(|(k, x)| format!("{k}:{}", go_sprint(x)))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        other => other.to_string_coerced(),
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
enum Diff {
    Full,
    Superset,
    No,
}

/// `nsf/jsondiff` result: `a` (actual) against `b` (expected).
/// ponytail: numbers compare by serde_json's Number, where jsondiff compares their literal text
/// (`1.0` vs `1.00` match here, not in Bento); explanation text is not jsondiff's coloured diff.
fn json_diff(a: &Json, b: &Json) -> Diff {
    match (a, b) {
        (Json::Object(ma), Json::Object(mb)) => {
            let mut d = Diff::Full;
            for (k, va) in ma {
                d = d.max(mb.get(k).map_or(Diff::Superset, |vb| json_diff(va, vb)));
            }
            if mb.keys().any(|k| !ma.contains_key(k)) {
                d = Diff::No;
            }
            d
        }
        (Json::Array(aa), Json::Array(ab)) => {
            if aa.len() < ab.len() {
                return Diff::No;
            }
            let mut d = if aa.len() > ab.len() {
                Diff::Superset
            } else {
                Diff::Full
            };
            for (x, y) in aa.iter().zip(ab) {
                d = d.max(json_diff(x, y));
            }
            d
        }
        (x, y) if x == y => Diff::Full,
        _ => Diff::No,
    }
}

fn first_json(raw: &[u8]) -> Option<Json> {
    serde_json::Deserializer::from_slice(raw)
        .into_iter::<Json>()
        .next()
        .and_then(Result::ok)
}

fn json_check(act: &[u8], exp: &str, superset_ok: bool) -> Result<(), String> {
    let head = if superset_ok {
        "JSON superset mismatch"
    } else {
        "JSON content mismatch"
    };
    let explanation = match (first_json(act), first_json(exp.as_bytes())) {
        (None, None) => "both arguments are invalid json".to_string(),
        (None, _) => "first argument is invalid json".to_string(),
        (_, None) => "second argument is invalid json".to_string(),
        (Some(a), Some(b)) => match json_diff(&a, &b) {
            Diff::Full => return Ok(()),
            Diff::Superset if superset_ok => return Ok(()),
            _ => format!("expected: {b}\nreceived: {a}"),
        },
    };
    Err(format!("{head}\n{explanation}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mock keeps the original's label unless it brings its own; a mock that is no object
    /// (a scalar) replaces the component as it is.
    #[test]
    fn set_mock_keeps_the_label_of_the_component_it_replaces() {
        let segs = |p: &str| pointer_segments(p);
        let mut root = serde_json::json!({"a": {"label": "orig", "x": 1}, "b": 2});
        set_mock(&mut root, &serde_json::json!({"y": 1}), &segs("/a")).expect("set");
        assert_eq!(root["a"], serde_json::json!({"label": "orig", "y": 1}));
        set_mock(&mut root, &serde_json::json!({"label": "own"}), &segs("/a")).expect("set");
        assert_eq!(root["a"], serde_json::json!({}), "a mock's own label is dropped");
        set_mock(&mut root, &serde_json::json!({"z": 1}), &segs("/b")).expect("set");
        assert_eq!(root["b"], serde_json::json!({"z": 1}), "no label to keep");
        set_mock(&mut root, &serde_json::json!("plain"), &segs("/b")).expect("set");
        assert_eq!(root["b"], "plain");
    }

    #[test]
    fn clean_matches_go() {
        assert_eq!(clean("./config/test/"), "config/test");
        assert_eq!(clean("a/../b/./c.yaml"), "b/c.yaml");
        assert_eq!(clean("/x//y/"), "/x/y");
        assert_eq!(clean("./"), ".");
    }

    #[test]
    fn path_pair_both_ways() {
        assert_eq!(
            path_pair("d/awk.yaml"),
            (
                "d/awk.yaml".to_string(),
                "d/awk_bento_test.yaml".to_string()
            )
        );
        assert_eq!(
            path_pair("d/awk_bento_test.yaml"),
            (
                "d/awk.yaml".to_string(),
                "d/awk_bento_test.yaml".to_string()
            )
        );
    }

    #[test]
    fn case_lines_follow_yaml_nodes() {
        let text = "pipeline: {}\ntests:\n  - name: a\n    x: 1\n  -\n    name: b\nother: 1\n";
        assert_eq!(case_lines(text), vec![3, 6]);
    }

    #[test]
    fn go_marshal_sorts_and_escapes() {
        let v: Json = serde_json::json!({"b": "<&>", "a": [1, 1.5]});
        assert_eq!(
            go_json_marshal(&v),
            r#"{"a":[1,1.5],"b":"\u003c\u0026\u003e"}"#
        );
    }

    #[test]
    fn json_diff_superset_rules() {
        let a = serde_json::json!({"a": 1, "b": [1, 2, 3], "c": true});
        let b = serde_json::json!({"a": 1, "b": [1, 2]});
        assert_eq!(json_diff(&a, &b), Diff::Superset);
        assert_eq!(json_diff(&b, &a), Diff::No);
        assert_eq!(json_diff(&a, &a), Diff::Full);
        assert_eq!(
            json_diff(&serde_json::json!(1), &serde_json::json!(1.0)),
            Diff::No
        );
    }

    fn write(dir: &Path, name: &str, text: &str) -> String {
        let p = dir.join(name);
        std::fs::create_dir_all(p.parent().unwrap_or(dir)).expect("dir");
        std::fs::write(&p, text).expect("write");
        p.to_string_lossy().into_owned()
    }

    async fn report(paths: &[PathBuf], resources: &[PathBuf]) -> (bool, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let ok = run_all(
            crate::build_full_registry(),
            paths,
            resources,
            &mut out,
            &mut err,
        )
        .await;
        (
            ok,
            String::from_utf8(out).expect("utf8"),
            String::from_utf8(err).expect("utf8"),
        )
    }

    const TARGET: &str = r#"pipeline:
  processors:
    - label: up
      mapping: 'root = content().uppercase()'
    - label: tag
      mapping: 'meta tag = "t"'
"#;

    #[tokio::test]
    async fn run_all_reports_passes_failures_and_missing_targets() {
        let dir = tempfile::tempdir().expect("dir");
        let d = dir.path();
        write(d, "ok.yaml", TARGET);
        write(
            d,
            "ok_bento_test.yaml",
            r#"tests:
  - name: upper
    input_batch:
      - content: abc
        metadata: {k: v}
    output_batches:
      - - content_equals: ABC
          metadata_equals: {tag: t, k: v}
          content_matches: '^A.C$'
          bloblang: 'content() == "ABC"'
"#,
        );
        write(d, "bad.yaml", TARGET);
        write(
            d,
            "bad_bento_test.yaml",
            r#"tests:
  - name: wrong
    input_batches:
      - - content: abc
        - content: def
    output_batches:
      - - content_equals: nope
  - name: counts
    input_batch:
      - content: x
    output_batches:
      - []
      - - content_equals: X
"#,
        );
        write(d, "sub/notes.txt", "not yaml");
        write(d, "sub/none.yaml", "input: {generate: {}}\n");
        let (ok, out, err) = report(&[PathBuf::from(format!("{}/...", d.display()))], &[]).await;
        assert!(!ok);
        assert!(err.is_empty(), "{err}");
        assert!(
            out.contains("Test '") && out.contains("ok.yaml' succeeded"),
            "{out}"
        );
        assert!(out.contains("bad.yaml' failed"), "{out}");
        assert!(out.contains("wrong [line 2]:"), "{out}");
        assert!(
            out.contains("content_equals: content mismatch\n  expected: nope\n  received: ABC"),
            "{out}"
        );
        assert!(
            out.contains("unexpected message from batch 0: DEF"),
            "{out}"
        );
        assert!(
            out.contains("mismatch of output batch 0 message counts, expected 1, got 2"),
            "{out}"
        );
        assert!(out.contains("counts [line 8]:"), "{out}");
        assert!(
            out.contains("wrong batch count, expected 2, got 1"),
            "{out}"
        );

        let (ok, out, _) = report(&[d.join("ok.yaml")], &[]).await;
        assert!(ok, "{out}");
        let (ok, out, _) = report(&[d.join("sub")], &[]).await;
        assert!(!ok);
        assert_eq!(out, "No tests were found\n");
        let (ok, _, err) = report(&[d.join("missing.yaml")], &[]).await;
        assert!(!ok);
        assert!(
            err.starts_with("Failed to obtain test targets: unable to access target config file"),
            "{err}"
        );
        let (ok, _, err) = report(&[PathBuf::from(format!("{}/gone/...", d.display()))], &[]).await;
        assert!(!ok);
        assert!(err.contains("gone"), "{err}");
    }

    #[tokio::test]
    async fn two_failing_targets_and_odd_paths_are_reported_in_order() {
        let dir = tempfile::tempdir().expect("dir");
        let d = dir.path();
        let failing = "pipeline:\n  processors:\n    - mapping: 'root = content()'\ntests:\n  - name: f\n    environment: ~\n    input_batch: [{content: a}]\n    output_batches: [[{content_equals: b}]]\n";
        write(d, "a.yaml", failing);
        write(d, "b.yaml", failing);
        write(d, "nested/c.yaml", failing);
        write(d, "notes.txt", "plain");
        let (ok, out, _) = report(&[d.to_path_buf(), d.join("notes.txt")], &[]).await;
        assert!(!ok);
        assert!(
            out.contains("a.yaml' failed") && out.contains("b.yaml' failed"),
            "{out}"
        );
        assert!(
            !out.contains("c.yaml"),
            "a directory without /... is not walked: {out}"
        );
        assert!(
            out.contains("---\n\nf [line 5]:\n") && out.contains("\n\n--- "),
            "{out}"
        );

        write(d, "x.yaml", TARGET);
        std::fs::create_dir(d.join("x_bento_test.yaml")).expect("dir");
        let (_, _, err) = report(&[d.join("x.yaml")], &[]).await;
        assert!(err.contains("failed to read test definition from"), "{err}");
        write(d, "y.yaml", TARGET);
        write(d, "y_bento_test.yaml", "tests: [\n");
        let (_, _, err) = report(&[d.join("y.yaml")], &[]).await;
        assert!(
            err.contains("failed to parse test definition from"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn a_target_that_does_not_build_stops_the_run() {
        let dir = tempfile::tempdir().expect("dir");
        let d = dir.path();
        write(
            d,
            "t.yaml",
            "pipeline:\n  processors:\n    - mapping: 'root = ('\ntests:\n  - name: x\n    input_batch: [{content: a}]\n",
        );
        let (ok, _, err) = report(&[d.join("t.yaml")], &[]).await;
        assert!(!ok);
        assert!(
            err.contains("Failed to execute test target") && err.contains("test case 0 failed"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn mappings_files_mocks_labels_and_resources_feed_a_case() {
        let dir = tempfile::tempdir().expect("dir");
        let d = dir.path();
        write(d, "lib.blobl", "map up {\n  root = this.uppercase()\n}\n");
        write(
            d,
            "m.blobl",
            "import \"./lib.blobl\"\nroot = content().string().apply(\"up\")\n",
        );
        write(d, "in.txt", "from file");
        write(d, "want.txt", "FROM FILE");
        write(d, "want.json", r#"{"a":1,"b":[1,2]}"#);
        write(
            d,
            "shared.yaml",
            "processor_resources:\n  - label: shout\n    mapping: 'root = content() + \"!\"'\n",
        );
        write(
            d,
            "t.yaml",
            r#"pipeline:
  processors:
    - label: call_api
      http:
        url: http://127.0.0.1:1/never
    - resource: shout
    - label: json
      mapping: 'root = {"a": 1, "b": [1, 2], "c": true}'
tests:
  - name: mapping file
    target_mapping: m.blobl
    input_batch:
      - file_content: in.txt
    output_batches:
      - - file_equals: want.txt
  - name: mocked by label and pointer
    target_processors: /pipeline/processors
    environment: {WHO: env}
    mocks:
      call_api:
        mapping: 'root = content().string() + "-" + env("WHO")'
      /pipeline/processors/2:
        label: replaced
        mapping: 'root = content()'
    input_batch:
      - content: hi
    output_batches:
      - - content_equals: hi-env!
  - name: json by label
    target_processors: json
    input_batch:
      - json_content: {x: 1}
    output_batches:
      - - json_equals: {a: 1, b: [1, 2], c: true}
          json_contains: '{"a": 1}'
          file_json_contains: want.json
  - name: whole file by fragment
    target_processors: 't.yaml#/pipeline/processors/2'
    input_batch: [{content: a}]
    output_batches:
      - - file_json_equals: want.json
"#,
        );
        let res = [d.join("shared.yaml")];
        let (ok, out, err) = report(&[d.join("t.yaml")], &res).await;
        assert!(err.is_empty(), "{err}");
        assert!(!ok, "{out}");
        // Only the last case fails: the result has `c`, the file does not.
        assert!(out.contains("whole file by fragment [line"), "{out}");
        assert!(
            out.contains("file_json_equals: JSON content mismatch"),
            "{out}"
        );
        assert!(!out.contains("mapping file ["), "{out}");
        assert!(!out.contains("mocked by label"), "{out}");
        assert!(!out.contains("json by label ["), "{out}");
    }

    #[tokio::test]
    async fn case_setup_errors_name_what_is_missing() {
        let dir = tempfile::tempdir().expect("dir");
        let d = dir.path();
        let target = write(d, "t.yaml", TARGET);
        let reg = crate::build_full_registry();
        let case = |extra: &str| {
            let def = format!("tests:\n  - name: c\n    input_batch: [{{content: a}}]\n{extra}");
            parse_definition(&def).expect("def").remove(0)
        };
        let bad_target = write(d, "bad_target.yaml", "input: 5\npipeline: {}\n");
        let broken_yaml = write(d, "broken_yaml.yaml", "pipeline: [\n");
        write(d, "imports.blobl", "import \"nope.blobl\"\nroot = this\n");
        for (extra, want) in [
            (
                "    target_mapping: gone.blobl\n",
                "failed to initialise Bloblang mapping 'gone.blobl'",
            ),
            (
                "    target_processors: 't.yaml#'\n",
                "reference URI must contain a path or fragment",
            ),
            (
                "    target_processors: nolabel\n",
                "target for label 'nolabel' failed",
            ),
            (
                "    target_processors: /pipeline/nothing\n",
                "failed to resolve case processors",
            ),
            (
                "    target_processors: /pipeline/processors/0/label\n",
                "failed to resolve case processors",
            ),
            (
                "    target_processors: 'bad_target.yaml#/pipeline'\n",
                &format!("failed to parse config file '{bad_target}': "),
            ),
            (
                "    target_processors: 'broken_yaml.yaml#/pipeline'\n",
                &format!("failed to parse config file '{broken_yaml}': "),
            ),
            (
                "    target_processors: 'other.yaml#/pipeline'\n",
                "failed to parse config file",
            ),
            ("    target_mapping: imports.blobl\n", "nope.blobl: "),
            (
                "    mocks: {nolabel: {noop: {}}}\n",
                "mock for label 'nolabel' could not be applied",
            ),
            (
                "    mocks: {/pipeline/processors/9/x: {noop: {}}}\n",
                "failed to set mock '/pipeline/processors/9/x'",
            ),
        ] {
            let err = execute(&reg, &[case(extra)], &target, &[])
                .await
                .expect_err(extra);
            assert!(err.contains(want), "{extra}: {err}");
        }
        write(d, "broken.blobl", "root = (");
        let err = execute(
            &reg,
            &[case("    target_mapping: broken.blobl\n")],
            &target,
            &[],
        )
        .await
        .expect_err("mapping");
        assert!(
            err.contains("failed to initialise Bloblang mapping 'broken.blobl'"),
            "{err}"
        );
        // `${VAR}` the case does not set is left for the config to report.
        let with_var = write(
            d,
            "var.yaml",
            "pipeline:\n  processors:\n    - mapping: 'root = \"${T7583_UNSET_VAR}\"'\n",
        );
        let failures = execute(
            &reg,
            &[case("    output_batches: [[{content_equals: x}]]\n")],
            &with_var,
            &[],
        )
        .await
        .expect("runs");
        assert_eq!(failures.len(), 1, "{failures:?}");
        let err = execute(&reg, &[case("")], &target, &[d.join("gone.yaml")])
            .await
            .expect_err("resources");
        assert!(err.contains("failed to parse config file"), "{err}");
        let bad_res = write(d, "bad_res.yaml", "input: 5\n");
        let err = execute(&reg, &[case("")], &target, &[PathBuf::from(bad_res)])
            .await
            .expect_err("bad resources");
        assert!(
            err.contains("failed to parse resources config file"),
            "{err}"
        );
        let bad = write(
            d,
            "badres.yaml",
            "processor_resources:\n  - label: x\n    mapping: 'root = ('\n",
        );
        let err = execute(&reg, &[case("")], &bad, &[])
            .await
            .expect_err("resources build");
        assert!(err.contains("failed to initialise resources"), "{err}");
        let file_case = parse_definition("tests:\n  - input_batch: [{file_content: none.txt}]\n")
            .expect("def")
            .remove(0);
        let err = execute(&reg, &[file_case], &target, &[])
            .await
            .expect_err("file");
        assert!(err.contains("failed to create test input 0"), "{err}");
    }

    #[tokio::test]
    async fn a_processor_error_and_an_extra_batch_are_reported() {
        let dir = tempfile::tempdir().expect("dir");
        let d = dir.path();
        let target = write(
            d,
            "t.yaml",
            "pipeline:\n  processors:\n    - mapping: 'root = this.missing.number()'\n",
        );
        let reg = crate::build_full_registry();
        let def = "tests:\n  - name: e\n    input_batches: [[{content: '{}'}], [{content: '{}'}]]\n    output_batches:\n      - - content_equals: x\n";
        let cases = parse_definition(def).expect("def");
        let failures = execute(&reg, &cases, &target, &[]).await.expect("runs");
        let reasons: Vec<&str> = failures.iter().map(|f| f.reason.as_str()).collect();
        assert!(
            reasons
                .iter()
                .any(|r| r.starts_with("batch 0 message 0: content_equals")),
            "{reasons:?}"
        );
        assert!(
            reasons
                .iter()
                .any(|r| r.starts_with("batch 0 message 0: ") && !r.contains("content_equals")),
            "{reasons:?}"
        );
        assert!(
            reasons.iter().any(|r| r.starts_with("unexpected batch: [")),
            "{reasons:?}"
        );
    }

    #[test]
    fn definitions_refuse_wrong_shapes() {
        assert!(parse_definition("a: [\n").is_err());
        assert!(parse_definition("pipeline: {}\n").expect("none").is_empty());
        assert_eq!(
            parse_definition("tests: 5\n").unwrap_err(),
            "field tests: expected array value"
        );
        for (def, want) in [
            (
                "tests: [{name: [1]}]\n",
                "field name: expected string value, got array",
            ),
            (
                "tests: [{environment: 5}]\n",
                "field environment: expected object value",
            ),
            (
                "tests: [{environment: {A: {b: 1}}}]\n",
                "expected string value, got object",
            ),
            (
                "tests: [{mocks: 5}]\n",
                "field mocks: expected object value",
            ),
            (
                "tests: [{input_batches: [5]}]\n",
                "field input_batches: expected array value",
            ),
            (
                "tests: [{output_batches: [5]}]\n",
                "field output_batches: expected array value",
            ),
            (
                "tests: [{input_batch: [{content: [1]}]}]\n",
                "expected string value, got array",
            ),
            (
                "tests: [{input_batch: [{file_content: {a: 1}}]}]\n",
                "expected string value, got object",
            ),
            (
                "tests: [{input_batch: [{metadata: 5}]}]\n",
                "field metadata: expected object value",
            ),
            (
                "tests: [{output_batches: [[{content_equals: [1]}]]}]\n",
                "field content_equals: expected string value",
            ),
            (
                "tests: [{output_batches: [[{bloblang: 'root = ('}]]}]\n",
                "bloblang: ",
            ),
            (
                "tests: [{output_batches: [[{metadata_equals: 5}]]}]\n",
                "field metadata_equals: expected object value",
            ),
        ] {
            let err = parse_definition(def).expect_err(def);
            assert!(err.contains(want), "{def}: {err}");
        }
        let c = parse_definition(
            "tests:\n  - name: 7\n    environment: {N: 1, B: true, Z: ~}\n    input_batch: []\n    output_batches: [[5, {unknown_check: x, json_equals: '{\"a\":1}'}]]\n",
        )
        .expect("def")
        .remove(0);
        assert_eq!(c.name, "7");
        assert_eq!(c.environment["N"], "1");
        assert_eq!(c.environment["B"], "true");
        assert_eq!(c.environment["Z"], "");
        assert!(c.input_batches.is_empty());
        assert!(c.output_batches[0][0].is_empty());
        assert_eq!(c.output_batches[0][1].len(), 1);
    }

    #[test]
    fn paths_split_join_and_clean_like_go() {
        assert_eq!(
            path_pair("plain"),
            ("plain".to_string(), "plain_bento_test".to_string())
        );
        assert_eq!(clean("../a/../../b"), "../../b");
        assert_eq!(clean("/../a"), "/a");
        assert_eq!(dir_of("/x"), "/");
        assert_eq!(dir_of("a/b/c"), "a/b");
        assert_eq!(dir_of("c"), ".");
        assert_eq!(join("a", "/abs/./x"), "/abs/x");
        assert_eq!(join("a", "b/../c"), "a/c");
        assert!(case_lines("pipeline: {}\n").is_empty());
        assert_eq!(
            case_lines("tests:\n# c\n\n- name: a\n  nested:\n    - x\n- name: b\n"),
            vec![4, 7]
        );
    }

    #[test]
    fn get_and_set_paths_refuse_what_is_not_there() {
        let mut root = serde_json::json!({"a": [{"b": 1}], "s": "x"});
        let seg = |p: &str| pointer_segments(p);
        assert_eq!(
            get_path(&root, &seg("/a/0/b")).unwrap(),
            &serde_json::json!(1)
        );
        assert_eq!(
            get_path(&root, &seg("/a/x")).unwrap_err(),
            "path 'x' not found"
        );
        assert_eq!(
            get_path(&root, &seg("/s/y")).unwrap_err(),
            "path 'y' not found"
        );
        assert_eq!(pointer_segments("/a~1b/c~0d"), vec!["a/b", "c~d"]);
        set_path(&mut root, &seg("/a/0/b"), serde_json::json!(2)).unwrap();
        set_path(&mut root, &seg("/new/deep"), serde_json::json!(3)).unwrap();
        assert_eq!(root["a"][0]["b"], 2);
        assert_eq!(root["new"]["deep"], 3);
        assert_eq!(
            set_path(&mut root, &seg("/a/5/b"), serde_json::json!(0)).unwrap_err(),
            "index '5' out of bounds"
        );
        assert_eq!(
            set_path(&mut root, &seg("/a/9"), serde_json::json!(0)).unwrap_err(),
            "index '9' out of bounds"
        );
        assert_eq!(
            set_path(&mut root, &seg("/s/t/u"), serde_json::json!(0)).unwrap_err(),
            "path 't' is not a container"
        );
        assert_eq!(
            set_path(&mut root, &seg("/s/t"), serde_json::json!(0)).unwrap_err(),
            "path 't' is not a container"
        );
        set_path(&mut root, &[], serde_json::json!("whole")).unwrap();
        assert_eq!(root, serde_json::json!("whole"));
        // A mock that names its own label drops it; a target without one adds none.
        let mut root = serde_json::json!({"p": [{"noop": {}}]});
        set_mock(
            &mut root,
            &serde_json::json!({"label": "m", "noop": {}}),
            &seg("/p/0"),
        )
        .unwrap();
        assert_eq!(root, serde_json::json!({"p": [{"noop": {}}]}));
        set_mock(&mut root, &serde_json::json!({"drop": {}}), &seg("/p/0")).unwrap();
        assert_eq!(root, serde_json::json!({"p": [{"drop": {}}]}));
        let labels = labels_to_paths(&serde_json::json!({"a": {"label": ""}, "b": {"label": "x"}}));
        assert_eq!(labels.len(), 1);
    }

    #[test]
    fn conditions_compare_content_metadata_and_json() {
        let dir = tempfile::tempdir().expect("dir");
        let d = dir.path().to_string_lossy().into_owned();
        let env = HashMap::new();
        let mut part = Part::new(r#"{"a":1,"b":[1,2]}"#);
        part.meta_set("s".to_string(), Value::String("v".into()));
        part.meta_set("n".to_string(), Value::Int(3));
        let chk = |c: Condition| check(&c, &d, &part, &env);
        assert!(chk(Condition::Bloblang("root = false".into()))
            .unwrap_err()
            .contains("was false"));
        assert!(chk(Condition::Bloblang("root = (".into())).is_err());
        assert!(chk(Condition::Bloblang("root = this.a.uppercase()".into())).is_err());
        assert!(chk(Condition::ContentMatches("(".into()))
            .unwrap_err()
            .starts_with("invalid pattern"));
        assert!(chk(Condition::ContentMatches("^x$".into()))
            .unwrap_err()
            .starts_with("pattern mismatch"));
        let meta =
            |k: &str, v: serde_json::Value| Condition::MetadataEquals(vec![(k.to_string(), v)]);
        assert!(chk(meta("n", serde_json::json!(3.0))).is_ok());
        assert!(chk(meta("gone", serde_json::json!(1)))
            .unwrap_err()
            .contains("expected but not found"));
        let err = chk(meta("s", serde_json::json!(["v"]))).unwrap_err();
        assert!(
            err.contains("expected: [v]") && err.contains("received: v"),
            "{err}"
        );
        assert!(chk(Condition::FileEquals("none.txt".into()))
            .unwrap_err()
            .starts_with("failed to read comparison file"));
        assert!(chk(Condition::FileJsonEquals("none.json".into()))
            .unwrap_err()
            .starts_with("failed to read comparison JSON file"));
        assert!(chk(Condition::JsonContains(r#"{"a":1}"#.into())).is_ok());
        assert!(chk(Condition::JsonEquals(r#"{"a":1}"#.into()))
            .unwrap_err()
            .starts_with("JSON content mismatch"));
        assert!(chk(Condition::JsonContains(r#"{"z":1}"#.into()))
            .unwrap_err()
            .starts_with("JSON superset mismatch"));
        assert!(chk(Condition::JsonEquals("nope".into()))
            .unwrap_err()
            .ends_with("second argument is invalid json"));
        assert!(json_check(b"x", "{}", false)
            .unwrap_err()
            .ends_with("first argument is invalid json"));
        assert!(json_check(b"x", "y", false)
            .unwrap_err()
            .ends_with("both arguments are invalid json"));
    }

    #[test]
    fn i_compare_and_go_sprint_follow_bento() {
        let v = |j: serde_json::Value| Value::from_json_value(j);
        assert!(i_compare(&Value::Null, &Value::Null));
        assert!(!i_compare(&Value::Null, &v(serde_json::json!(1))));
        assert!(i_compare(
            &v(serde_json::json!("b")),
            &Value::Bytes(b"b".to_vec())
        ));
        assert!(!i_compare(&v(serde_json::json!("b")), &Value::Int(1)));
        assert!(!i_compare(&Value::Int(1), &v(serde_json::json!("1"))));
        assert!(i_compare(
            &v(serde_json::json!(true)),
            &v(serde_json::json!(true))
        ));
        assert!(!i_compare(
            &v(serde_json::json!(true)),
            &v(serde_json::json!(false))
        ));
        assert!(i_compare(
            &v(serde_json::json!([1, "a"])),
            &v(serde_json::json!([1.0, "a"]))
        ));
        assert!(!i_compare(
            &v(serde_json::json!([1])),
            &v(serde_json::json!([1, 2]))
        ));
        assert!(!i_compare(
            &v(serde_json::json!([1])),
            &v(serde_json::json!({"a": 1}))
        ));
        assert!(i_compare(
            &v(serde_json::json!({"a": [1]})),
            &v(serde_json::json!({"a": [1]}))
        ));
        assert!(!i_compare(
            &v(serde_json::json!({"a": 1})),
            &v(serde_json::json!({"b": 1}))
        ));
        assert!(!i_compare(
            &v(serde_json::json!({"a": 1})),
            &v(serde_json::json!([1]))
        ));
        assert!(i_compare(
            &Value::Bytes(b"x".to_vec()),
            &Value::Bytes(b"x".to_vec())
        ));
        assert_eq!(go_sprint(&Value::Null), "<nil>");
        assert_eq!(
            go_sprint(&v(serde_json::json!({"k": [1, null]}))),
            "map[k:[1 <nil>]]"
        );
        assert_eq!(type_name(&serde_json::json!(null)), "null");
        assert_eq!(type_name(&serde_json::json!(true)), "bool");
        assert_eq!(type_name(&serde_json::json!(1)), "number");
        assert_eq!(type_name(&serde_json::json!("s")), "string");
        assert_eq!(
            json_test_string(&serde_json::json!({"b": 1, "a": 2})),
            r#"{"a":2,"b":1}"#
        );
    }

    #[test]
    fn mocks_keep_label_and_append() {
        let mut root = serde_json::json!({"pipeline": {"processors": [
            {"label": "x", "http": {"url": "u"}}
        ]}});
        let labels = labels_to_paths(&root);
        set_mock(
            &mut root,
            &serde_json::json!({"mapping": "root = 1"}),
            &labels["x"],
        )
        .unwrap();
        set_mock(
            &mut root,
            &serde_json::json!({"noop": {}}),
            &pointer_segments("/pipeline/processors/-"),
        )
        .unwrap();
        assert_eq!(
            root,
            serde_json::json!({"pipeline": {"processors": [
                {"mapping": "root = 1", "label": "x"}, {"noop": {}}
            ]}})
        );
    }
}
