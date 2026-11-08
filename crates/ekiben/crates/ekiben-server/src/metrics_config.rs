//! The metrics configuration of `streams`: a `metrics:` section from `--metrics <file>` or a
//! `-r/--resources` file (a whole config or the section alone), then each
//! `--set metrics.<path>=<value>` in order (the value read as YAML, so `true`, numbers and
//! `[0.1, 1]` keep their type), as Bento's runner flags set them.

use std::path::PathBuf;

use ekiben_core::MetricsConfig;
use serde_json::Value as JsonValue;

fn read_section(path: &PathBuf) -> Result<Option<JsonValue>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read '{}': {e}", path.display()))?;
    let value: JsonValue =
        serde_norway::from_str(&text).map_err(|e| format!("'{}': {e}", path.display()))?;
    Ok(value.get("metrics").cloned())
}

/// Applies one `--set` argument (`metrics.prometheus.use_histogram_timing=true`) to `root`
/// (the whole config tree); arguments outside `metrics.` are ignored here.
pub fn apply_set(root: &mut JsonValue, arg: &str) -> Result<(), String> {
    let (path, raw) = arg
        .split_once('=')
        .ok_or_else(|| format!("--set '{arg}': expected <path>=<value>"))?;
    let path = path.trim();
    if !path.starts_with("metrics.") && path != "metrics" {
        return Ok(());
    }
    let value: JsonValue = if raw.is_empty() {
        JsonValue::String(String::new())
    } else {
        serde_norway::from_str(raw).unwrap_or_else(|_| JsonValue::String(raw.to_string()))
    };
    let mut cur = root;
    let keys: Vec<&str> = path.split('.').collect();
    let Some((last, parents)) = keys.split_last() else {
        return Err(format!("--set '{arg}': empty path"));
    };
    for k in parents {
        if !cur.is_object() {
            *cur = JsonValue::Object(serde_json::Map::new());
        }
        let map = cur
            .as_object_mut()
            .ok_or_else(|| format!("--set '{arg}': '{k}' is not a map"))?;
        cur = map
            .entry((*k).to_string())
            .or_insert_with(|| JsonValue::Object(serde_json::Map::new()));
    }
    if !cur.is_object() {
        *cur = JsonValue::Object(serde_json::Map::new());
    }
    if let Some(map) = cur.as_object_mut() {
        // `mapping` stays a string whatever it looks like.
        let value = if *last == "mapping" {
            JsonValue::String(raw.to_string())
        } else {
            value
        };
        map.insert((*last).to_string(), value);
    }
    Ok(())
}

/// The metrics config from the files and `--set` arguments (later ones win).
pub fn load_metrics_config(
    metrics_file: Option<&PathBuf>,
    resources: &[PathBuf],
    sets: &[String],
) -> Result<MetricsConfig, String> {
    let mut root = serde_json::json!({});
    let mut files: Vec<&PathBuf> = resources.iter().collect();
    if let Some(f) = metrics_file {
        files.push(f);
    }
    for f in files {
        if let Some(section) = read_section(f)? {
            if let Some(map) = root.as_object_mut() {
                map.insert("metrics".to_string(), section);
            }
        } else if Some(f) == metrics_file {
            // A `--metrics` file may hold the section alone.
            let text = std::fs::read_to_string(f)
                .map_err(|e| format!("failed to read '{}': {e}", f.display()))?;
            let value: JsonValue =
                serde_norway::from_str(&text).map_err(|e| format!("'{}': {e}", f.display()))?;
            if let Some(map) = root.as_object_mut() {
                map.insert("metrics".to_string(), value);
            }
        }
    }
    for s in sets {
        apply_set(&mut root, s)?;
    }
    let section = root.get("metrics").cloned().unwrap_or(JsonValue::Null);
    MetricsConfig::from_json(&section).map_err(|e| e.to_string())
}
