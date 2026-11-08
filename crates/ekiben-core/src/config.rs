use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::error::CoreError;
use crate::interpolate::{interpolate_env_str, EnvLintError};
use crate::org::OrgId;

/// Stream execution mode: resident (`streams` mode default) or `once` (runs until input ends, frees runtime state).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    #[default]
    Resident,
    Once,
}

/// Overlap policy for scheduled streams (`Section 6`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlapPolicy {
    #[default]
    Skip,
    Queue,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ScheduleConfig {
    pub cron: Option<String>,
    pub period: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub overlap: OverlapPolicy,
    #[serde(default)]
    pub jitter: Option<String>,
}

/// Generic component configuration (`Input`, `Output`, `Processor`, `Cache`, `RateLimit`, `Buffer`).
/// Follows Bento's type inference rules (`internal/component/processor/config.go`, `input/config.go`, `output/config.go`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentConfig {
    #[serde(default)]
    pub label: String,
    #[serde(rename = "type")]
    pub component_type: String,
    #[serde(default)]
    pub config: JsonValue,
    #[serde(default)]
    pub processors: Vec<ComponentConfig>,
}

pub type InputConfig = ComponentConfig;
pub type OutputConfig = ComponentConfig;
pub type ProcessorConfig = ComponentConfig;
pub type BufferConfig = ComponentConfig;

impl Default for ComponentConfig {
    fn default() -> Self {
        Self {
            label: String::new(),
            component_type: "noop".to_string(),
            config: JsonValue::Object(serde_json::Map::new()),
            processors: Vec::new(),
        }
    }
}

impl ComponentConfig {
    /// Parses a Bento component YAML/JSON object using Bento's key inference rules:
    /// - Reserved keys on inputs/outputs/processors are `label`, `type`, `plugin`, `processors`.
    /// - If `type` is explicitly set, config is taken from `obj[type]` or `obj["plugin"]` or the remaining fields.
    /// - Otherwise, the single non-reserved key names the `component_type` and its value is `config`.
    pub fn from_json_value(
        val: &JsonValue,
        default_type: &str,
        known_types: Option<&dyn Fn(&str) -> bool>,
    ) -> Result<Self, CoreError> {
        let Some(map) = val.as_object() else {
            return Err(CoreError::config(format!(
                "expected object for component config, got {val}"
            )));
        };

        if map.is_empty() {
            return Ok(Self {
                label: String::new(),
                component_type: default_type.to_string(),
                config: JsonValue::Object(serde_json::Map::new()),
                processors: Vec::new(),
            });
        }

        let label = map
            .get("label")
            .and_then(JsonValue::as_str)
            .unwrap_or("")
            .to_string();

        let mut processors = Vec::new();
        if let Some(proc_val) = map.get("processors") {
            // Note: `processors` can also be the `processors` composite processor itself if there is no other type key!
            let has_other_candidate = map
                .keys()
                .any(|k| !matches!(k.as_str(), "label" | "processors"));
            if has_other_candidate {
                let Some(arr) = proc_val.as_array() else {
                    return Err(CoreError::config(
                        "processors field must be an array".to_string(),
                    ));
                };
                for p in arr {
                    processors.push(Self::from_json_value(p, "noop", known_types)?);
                }
            }
        }

        if let Some(explicit_type) = map.get("type").and_then(JsonValue::as_str) {
            // Wait: what if `type` is inside a `metric` processor inline? Component top-level only has `type` when it's a known component type or `plugin` is present.
            if known_types.is_none_or(|f| f(explicit_type)) || map.contains_key("plugin") {
                let cfg = map
                    .get(explicit_type)
                    .or_else(|| map.get("plugin"))
                    .cloned()
                    .unwrap_or_else(|| {
                        let mut rest = map.clone();
                        rest.remove("label");
                        rest.remove("type");
                        rest.remove("processors");
                        JsonValue::Object(rest)
                    });
                return Ok(Self {
                    label,
                    component_type: explicit_type.to_string(),
                    config: cfg,
                    processors,
                });
            }
        }

        // Find the component type key among non-reserved keys
        let mut candidates: Vec<&String> = map
            .keys()
            .filter(|k| !matches!(k.as_str(), "label" | "processors"))
            .collect();

        if candidates.is_empty() && map.contains_key("processors") {
            // This is the `processors: [...]` composite processor!
            return Ok(Self {
                label,
                component_type: "processors".to_string(),
                config: map["processors"].clone(),
                processors: Vec::new(),
            });
        }

        if candidates.len() > 1 {
            if let Some(check_fn) = known_types {
                let matched: Vec<&String> =
                    candidates.iter().copied().filter(|k| check_fn(k)).collect();
                if matched.len() == 1 {
                    candidates = matched;
                }
            }
        }

        if candidates.len() != 1 {
            return Err(CoreError::config(format!(
                "unable to infer component type from keys: {:?}",
                map.keys().collect::<Vec<_>>()
            )));
        }

        let comp_type = candidates[0].clone();
        let cfg = map.get(&comp_type).cloned().unwrap_or(JsonValue::Null);

        Ok(Self {
            label,
            component_type: comp_type,
            config: cfg,
            processors,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PipelineConfig {
    #[serde(default = "default_pipeline_threads")]
    pub threads: isize,
    #[serde(default)]
    pub processors: Vec<ProcessorConfig>,
}

fn default_pipeline_threads() -> isize {
    -1
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            threads: -1,
            processors: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BatchPolicyConfig {
    #[serde(default)]
    pub count: usize,
    #[serde(default)]
    pub byte_size: usize,
    #[serde(default)]
    pub period: String,
    #[serde(default)]
    pub jitter: f64,
    #[serde(default)]
    pub check: String,
    #[serde(default)]
    pub processors: Vec<ProcessorConfig>,
}

impl BatchPolicyConfig {
    pub fn is_noop(&self) -> bool {
        self.byte_size == 0
            && self.count <= 1
            && self.check.is_empty()
            && self.period.is_empty()
            && self.processors.is_empty()
    }

    pub fn is_limited(&self) -> bool {
        self.byte_size > 0 || self.count > 0 || !self.period.is_empty() || !self.check.is_empty()
    }

    pub fn from_json_value(val: &JsonValue) -> Result<Self, CoreError> {
        let Some(map) = val.as_object() else {
            return Ok(Self::default());
        };
        let count = map.get("count").and_then(JsonValue::as_u64).unwrap_or(0) as usize;
        let byte_size = map
            .get("byte_size")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0) as usize;
        let period = map
            .get("period")
            .and_then(JsonValue::as_str)
            .unwrap_or("")
            .to_string();
        let jitter = map.get("jitter").and_then(JsonValue::as_f64).unwrap_or(0.0);
        if jitter < 0.0 {
            return Err(CoreError::config("jitter factor cannot be negative"));
        }
        let check = map
            .get("check")
            .and_then(JsonValue::as_str)
            .unwrap_or("")
            .to_string();
        let mut processors = Vec::new();
        if let Some(arr) = map.get("processors").and_then(JsonValue::as_array) {
            for p in arr {
                processors.push(ComponentConfig::from_json_value(p, "noop", None)?);
            }
        }
        Ok(Self {
            count,
            byte_size,
            period,
            jitter,
            check,
            processors,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackoffConfig {
    pub initial_interval: Duration,
    pub max_interval: Duration,
    pub max_elapsed_time: Duration,
}

impl Default for BackoffConfig {
    fn default() -> Self {
        Self {
            initial_interval: Duration::from_millis(500),
            max_interval: Duration::from_secs(3),
            max_elapsed_time: Duration::from_secs(0),
        }
    }
}

impl BackoffConfig {
    pub fn from_json_value(
        val: &JsonValue,
        default_max_elapsed: Duration,
    ) -> Result<Self, CoreError> {
        let mut cfg = Self {
            max_elapsed_time: default_max_elapsed,
            ..Default::default()
        };
        if let Some(map) = val.as_object() {
            if let Some(s) = map.get("initial_interval").and_then(JsonValue::as_str) {
                cfg.initial_interval = parse_duration_str(s)?;
            }
            if let Some(s) = map.get("max_interval").and_then(JsonValue::as_str) {
                cfg.max_interval = parse_duration_str(s)?;
            }
            if let Some(s) = map.get("max_elapsed_time").and_then(JsonValue::as_str) {
                cfg.max_elapsed_time = parse_duration_str(s)?;
            }
        }
        Ok(cfg)
    }

    pub fn next_backoff(&self, attempt: u32, total_elapsed: Duration) -> Option<Duration> {
        if !self.max_elapsed_time.is_zero() && total_elapsed >= self.max_elapsed_time {
            return None;
        }
        let factor = 2u64.saturating_pow(attempt.min(20));
        let wait = self
            .initial_interval
            .saturating_mul(factor as u32)
            .min(self.max_interval);
        Some(wait)
    }
}

pub fn parse_duration_str(s: &str) -> Result<Duration, CoreError> {
    if s.trim().is_empty() || s.trim() == "0" || s.trim() == "0s" {
        return Ok(Duration::ZERO);
    }
    let nanos = time_format_dur(s).map_err(CoreError::config)?;
    if nanos <= 0 {
        return Ok(Duration::ZERO);
    }
    Ok(Duration::from_nanos(nanos as u64))
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ResourceConfigs {
    #[serde(default)]
    pub cache_resources: Vec<ComponentConfig>,
    #[serde(default)]
    pub rate_limit_resources: Vec<ComponentConfig>,
    #[serde(default)]
    pub processor_resources: Vec<ComponentConfig>,
    #[serde(default)]
    pub input_resources: Vec<ComponentConfig>,
    #[serde(default)]
    pub output_resources: Vec<ComponentConfig>,
}

/// Complete stream configuration (`internal/stream/config.go`), extended with optional
/// `org`, `project`, `run` (`resident` | `once`), `schedule`, and `secret_refs`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StreamConfig {
    #[serde(default)]
    pub org: OrgId,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub run: RunMode,
    #[serde(default)]
    pub schedule: Option<ScheduleConfig>,
    #[serde(default)]
    pub secret_refs: Vec<String>,
    pub input: InputConfig,
    pub buffer: BufferConfig,
    pub pipeline: PipelineConfig,
    pub output: OutputConfig,
    #[serde(flatten)]
    pub resources: ResourceConfigs,
    #[serde(skip)]
    pub raw_source: JsonValue,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            org: OrgId::default(),
            project: None,
            run: RunMode::Resident,
            schedule: None,
            secret_refs: Vec::new(),
            input: ComponentConfig {
                label: String::new(),
                component_type: "stdin".to_string(),
                config: JsonValue::Object(serde_json::Map::new()),
                processors: Vec::new(),
            },
            buffer: ComponentConfig {
                label: String::new(),
                component_type: "none".to_string(),
                config: JsonValue::Object(serde_json::Map::new()),
                processors: Vec::new(),
            },
            pipeline: PipelineConfig::default(),
            output: ComponentConfig {
                label: String::new(),
                component_type: "stdout".to_string(),
                config: JsonValue::Object(serde_json::Map::new()),
                processors: Vec::new(),
            },
            resources: ResourceConfigs::default(),
            raw_source: JsonValue::Object(serde_json::Map::new()),
        }
    }
}

impl StreamConfig {
    /// Parses a YAML or JSON stream configuration, performing `${VAR:default}` interpolation first.
    /// If `chilled` is `false`, undefined `${VAR}` references return a lint error.
    pub fn parse_str_with_env<F>(
        raw_text: &str,
        chilled: bool,
        lookup_env: F,
    ) -> Result<(Self, Vec<String>), CoreError>
    where
        F: FnMut(&str) -> Option<String>,
    {
        let mut env_lints = Vec::new();
        let interpolated = match interpolate_env_str(raw_text, lookup_env) {
            Ok(s) => s,
            Err(EnvLintError {
                missing,
                best_attempt,
            }) => {
                for m in missing {
                    env_lints.push(format!("environment variable '{m}' was not defined"));
                }
                if !chilled {
                    return Err(CoreError::config(env_lints.join("; ")));
                }
                best_attempt
            }
        };

        let raw_json: JsonValue = serde_norway::from_str(&interpolated)
            .map_err(|e| CoreError::config(format!("failed to parse YAML/JSON config: {e}")))?;
        let cfg = Self::from_json_value(&raw_json)?;
        Ok((cfg, env_lints))
    }

    /// Parses with `${VAR}` filled from the whole process environment ([`EnvAccess::Process`]):
    /// for tests and a person's own configs only; servers use [`Self::parse_str_with_env`] with
    /// their access.
    pub fn from_yaml_or_json(raw_text: &str) -> Result<Self, CoreError> {
        let (cfg, _) = Self::parse_str_with_env(raw_text, false, |k| EnvAccess::Process.lookup(k))?;
        Ok(cfg)
    }

    /// Reads the `org` (or `org_id`) field of a config text without interpolating it, so the
    /// organization whose variables fill `${VAR}` is known before the substitution.
    pub fn peek_org(raw_text: &str) -> OrgId {
        serde_norway::from_str::<JsonValue>(raw_text)
            .ok()
            .as_ref()
            .and_then(|v| v.get("org").or_else(|| v.get("org_id")))
            .and_then(JsonValue::as_str)
            .map(OrgId::new)
            .unwrap_or_default()
    }

    /// The config as written, before `${VAR}` interpolation, as a JSON value (what
    /// `GET /streams/{id}` answers after redaction); `None` when the raw text does not parse.
    pub fn posted_json(raw_text: &str) -> Option<JsonValue> {
        serde_norway::from_str::<JsonValue>(raw_text).ok()
    }

    pub fn from_json_value(raw_json: &JsonValue) -> Result<Self, CoreError> {
        let Some(map) = raw_json.as_object() else {
            return Err(CoreError::config("stream configuration must be an object"));
        };

        let org = map
            .get("org")
            .or_else(|| map.get("org_id"))
            .and_then(JsonValue::as_str)
            .map(OrgId::new)
            .unwrap_or_default();

        let project = map
            .get("project")
            .and_then(JsonValue::as_str)
            .map(String::from);

        let run = match map.get("run").and_then(JsonValue::as_str) {
            Some("once") => RunMode::Once,
            _ => RunMode::Resident,
        };

        let schedule = match map.get("schedule") {
            Some(JsonValue::String(s)) => {
                if s.starts_with("@every ")
                    || s.ends_with('s')
                    || s.ends_with('m')
                    || s.ends_with('h')
                {
                    Some(ScheduleConfig {
                        cron: if s.starts_with("@every ") {
                            Some(s.clone())
                        } else {
                            None
                        },
                        period: if !s.starts_with("@every ") {
                            Some(s.clone())
                        } else {
                            None
                        },
                        ..Default::default()
                    })
                } else {
                    Some(ScheduleConfig {
                        cron: Some(s.clone()),
                        ..Default::default()
                    })
                }
            }
            Some(obj @ JsonValue::Object(_)) => serde_json::from_value(obj.clone())
                .map_err(|e| CoreError::config(format!("invalid schedule config: {e}")))?,
            _ => None,
        };

        let secret_refs = map
            .get("secret_refs")
            .and_then(JsonValue::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(JsonValue::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();

        let input = match map.get("input") {
            Some(v) => ComponentConfig::from_json_value(v, "stdin", None)?,
            None => ComponentConfig {
                label: String::new(),
                component_type: "inproc".to_string(),
                config: JsonValue::String(String::new()),
                processors: Vec::new(),
            },
        };

        let buffer = match map.get("buffer") {
            Some(v) => ComponentConfig::from_json_value(v, "none", None)?,
            None => ComponentConfig {
                label: String::new(),
                component_type: "none".to_string(),
                config: JsonValue::Object(serde_json::Map::new()),
                processors: Vec::new(),
            },
        };

        let mut pipeline = PipelineConfig::default();
        if let Some(p_obj) = map.get("pipeline").and_then(JsonValue::as_object) {
            if let Some(t) = p_obj.get("threads").and_then(JsonValue::as_i64) {
                pipeline.threads = t as isize;
            }
            if let Some(procs) = p_obj.get("processors").and_then(JsonValue::as_array) {
                for p in procs {
                    pipeline
                        .processors
                        .push(ComponentConfig::from_json_value(p, "noop", None)?);
                }
            }
        }

        let output = match map.get("output") {
            Some(v) => ComponentConfig::from_json_value(v, "stdout", None)?,
            None => ComponentConfig {
                label: String::new(),
                component_type: "stdout".to_string(),
                config: JsonValue::Object(serde_json::Map::new()),
                processors: Vec::new(),
            },
        };

        let parse_res_list = |key: &str| -> Result<Vec<ComponentConfig>, CoreError> {
            let Some(arr) = map.get(key).and_then(JsonValue::as_array) else {
                return Ok(Vec::new());
            };
            arr.iter()
                .map(|v| ComponentConfig::from_json_value(v, "noop", None))
                .collect()
        };

        let resources = ResourceConfigs {
            cache_resources: parse_res_list("cache_resources")?,
            rate_limit_resources: parse_res_list("rate_limit_resources")?,
            processor_resources: parse_res_list("processor_resources")?,
            input_resources: parse_res_list("input_resources")?,
            output_resources: parse_res_list("output_resources")?,
        };

        Ok(Self {
            org,
            project,
            run,
            schedule,
            secret_refs,
            input,
            buffer,
            pipeline,
            output,
            resources,
            raw_source: raw_json.clone(),
        })
    }
}

pub fn extract_metadata_filter(val: &JsonValue) -> MetadataFilter {
    let mut filter = MetadataFilter::default();
    if let Some(map) = val.as_object() {
        if let Some(arr) = map.get("include_prefixes").and_then(JsonValue::as_array) {
            filter.include_prefixes = arr
                .iter()
                .filter_map(JsonValue::as_str)
                .map(String::from)
                .collect();
        }
        if let Some(arr) = map.get("include_patterns").and_then(JsonValue::as_array) {
            for pat in arr.iter().filter_map(JsonValue::as_str) {
                if let Ok(re) = regex::Regex::new(pat) {
                    filter.include_patterns.push(re);
                }
            }
        }
        if let Some(arr) = map.get("exclude_prefixes").and_then(JsonValue::as_array) {
            filter.exclude_prefixes = arr
                .iter()
                .filter_map(JsonValue::as_str)
                .map(String::from)
                .collect();
        }
    }
    filter
}

#[derive(Clone, Debug, Default)]
pub struct MetadataFilter {
    pub include_prefixes: Vec<String>,
    pub include_patterns: Vec<regex::Regex>,
    pub exclude_prefixes: Vec<String>,
}

impl MetadataFilter {
    pub fn is_empty(&self) -> bool {
        self.include_prefixes.is_empty() && self.include_patterns.is_empty()
    }

    pub fn matches(&self, key: &str) -> bool {
        if self
            .exclude_prefixes
            .iter()
            .any(|p| !p.is_empty() && key.starts_with(p))
        {
            return false;
        }
        if self
            .include_prefixes
            .iter()
            .any(|p| p == "*" || key.starts_with(p))
        {
            return true;
        }
        if self.include_patterns.iter().any(|re| re.is_match(key)) {
            return true;
        }
        false
    }
}

pub(crate) fn time_format_dur(s: &str) -> Result<i64, String> {
    ekiben_bloblang::time_format::parse_go_duration_nanos(s)
}

pub fn merge_json_patch(dst: &mut JsonValue, patch: &JsonValue) {
    match (dst, patch) {
        (JsonValue::Object(d_map), JsonValue::Object(p_map)) => {
            for (k, pv) in p_map {
                match d_map.get_mut(k) {
                    Some(dv) if dv.is_object() && pv.is_object() => {
                        merge_json_patch(dv, pv);
                    }
                    _ => {
                        d_map.insert(k.clone(), pv.clone());
                    }
                }
            }
        }
        (d, p) => *d = p.clone(),
    }
}

/// Prefix of the process environment variables a stream may read when access is
/// [`EnvAccess::AllowListed`].
pub const ENV_VAR_ALLOW_PREFIX: &str = "EKIBEN_VAR_";

/// Which process environment variables a stream configuration can read, through `${VAR}`
/// substitution of a config and through Bloblang `env()` alike.
///
/// Under [`EnvAccess::AllowListed`] `${NAME}` resolves in this order: the stream organization's
/// variable map ([`EnvAccess::resolve`]); the process variable `EKIBEN_VAR_<NAME>` when `NAME`
/// lacks the prefix, or `NAME` when it has it; else unset (a lint error unless `${NAME:default}`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EnvAccess {
    /// No process variable: only the organization's map and the stream's own secrets.
    #[default]
    None,
    /// Only variables named `EKIBEN_VAR_*`: `streams`, `schedule` and `run` (the mode of a shared
    /// process such as `jc`, whose environment also holds other modules' secrets).
    AllowListed,
    /// The whole process environment (Bento's behaviour): `run --host-env` and `blobl`, a
    /// person's own shell.
    Process,
}

impl From<bool> for EnvAccess {
    /// `true` keeps the old `allow_host_env` meaning (whole process environment), `false` none.
    fn from(allow_host_env: bool) -> Self {
        if allow_host_env {
            EnvAccess::Process
        } else {
            EnvAccess::None
        }
    }
}

/// The process variable an allow-listed `${NAME}` reads: `EKIBEN_VAR_NAME`, or `NAME` itself when
/// it already carries the prefix.
pub fn allow_listed_name(name: &str) -> String {
    if name.starts_with(ENV_VAR_ALLOW_PREFIX) {
        name.to_string()
    } else {
        format!("{ENV_VAR_ALLOW_PREFIX}{name}")
    }
}

impl EnvAccess {
    /// Looks up `name` in the process environment as this access allows it.
    pub fn lookup(self, name: &str) -> Option<String> {
        match self {
            EnvAccess::None => None,
            EnvAccess::AllowListed => std::env::var(allow_listed_name(name)).ok(),
            EnvAccess::Process => std::env::var(name).ok(),
        }
    }

    /// The whole resolution of `${NAME}` for a stream: the organization's variable map first,
    /// then the process environment as this access allows.
    pub fn resolve(self, org_vars: &HashMap<String, String>, name: &str) -> Option<String> {
        org_vars.get(name).cloned().or_else(|| self.lookup(name))
    }
}

/// The Bloblang environment of a stream: `env()` reads `env_vars` (the organization's variable
/// map and the stream's secrets) first, then the process environment as `access` allows.
pub fn build_blobl_env_for_stream(
    env_vars: &HashMap<String, String>,
    access: impl Into<EnvAccess>,
) -> ekiben_bloblang::Environment {
    let env = ekiben_bloblang::Environment::new().with_env_vars(env_vars.clone());
    match access.into() {
        EnvAccess::None => env.with_host_env(false),
        EnvAccess::AllowListed => env
            .with_host_env(true)
            .with_host_env_prefix(ENV_VAR_ALLOW_PREFIX),
        EnvAccess::Process => env.with_host_env(true),
    }
}
