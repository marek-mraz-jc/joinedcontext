//! The metrics configuration of `streams`: a `metrics:` section from `--metrics <file>` or a
//! `-r/--resources` file (a whole config or the section alone), then each
//! `--set metrics.<path>=<value>` in order (the value read as YAML, so `true`, numbers and
//! `[0.1, 1]` keep their type), as Bento's runner flags set them.

use std::path::PathBuf;

use ekiben_core::MetricsConfig;
use serde_json::Value as JsonValue;

fn read_yaml(path: &PathBuf) -> Result<JsonValue, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read '{}': {e}", path.display()))?;
    serde_norway::from_str(&text).map_err(|e| format!("'{}': {e}", path.display()))
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
    // `mapping` stays a string whatever it looks like.
    let (parents, last) = match path.rsplit_once('.') {
        Some((parents, last)) => (parents.split('.').collect(), last),
        None => (Vec::new(), path),
    };
    let value = if last == "mapping" {
        JsonValue::String(raw.to_string())
    } else {
        value
    };
    let mut cur = root;
    for k in parents {
        if !cur.is_object() {
            *cur = serde_json::json!({});
        }
        cur = &mut cur[k];
    }
    if !cur.is_object() {
        *cur = serde_json::json!({});
    }
    cur[last] = value;
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
        let value = read_yaml(f)?;
        // A `--metrics` file may hold the section alone.
        let section = match value.get("metrics") {
            Some(section) => Some(section.clone()),
            None if Some(f) == metrics_file => Some(value),
            None => None,
        };
        if let Some(section) = section {
            root["metrics"] = section;
        }
    }
    for s in sets {
        apply_set(&mut root, s)?;
    }
    let section = root.get("metrics").cloned().unwrap_or(JsonValue::Null);
    MetricsConfig::from_json(&section).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn set(root: &mut JsonValue, arg: &str) {
        apply_set(root, arg).expect(arg);
    }

    #[test]
    fn set_keeps_yaml_types_and_mapping_text() {
        let mut root = json!({});
        set(&mut root, "metrics.prometheus.use_histogram_timing=true");
        set(&mut root, "metrics.prometheus.histogram_buckets=[0.1, 1]");
        set(&mut root, "metrics.mapping=root = this.uppercase()");
        set(&mut root, "metrics.prometheus.prefix=");
        set(&mut root, "metrics.prometheus.label=a: [");
        assert_eq!(
            root,
            json!({"metrics": {
                "mapping": "root = this.uppercase()",
                "prometheus": {
                    "use_histogram_timing": true,
                    "histogram_buckets": [0.1, 1],
                    "prefix": "",
                    "label": "a: [",
                }
            }})
        );
    }

    #[test]
    fn set_outside_metrics_is_ignored_and_a_missing_equals_is_refused() {
        let mut root = json!({});
        set(&mut root, "input.generate.mapping=root = 1");
        set(&mut root, "metricsx.a=1");
        assert_eq!(root, json!({}));
        let err = apply_set(&mut root, "metrics.prometheus").expect_err("no =");
        assert!(err.contains("expected <path>=<value>"), "{err}");
    }

    #[test]
    fn set_replaces_a_scalar_on_its_path_with_a_map() {
        let mut root = json!({"metrics": 5});
        set(&mut root, "metrics.prometheus.prefix=x");
        assert_eq!(root, json!({"metrics": {"prometheus": {"prefix": "x"}}}));
        let mut root = json!({"metrics": {"prometheus": "off"}});
        set(&mut root, "metrics.prometheus.prefix=y");
        assert_eq!(root, json!({"metrics": {"prometheus": {"prefix": "y"}}}));
        let mut root = json!(7);
        set(&mut root, "metrics=none");
        assert_eq!(root, json!({"metrics": "none"}));
    }

    fn file(dir: &tempfile::TempDir, name: &str, text: &str) -> PathBuf {
        let p = dir.path().join(name);
        std::fs::write(&p, text).expect("write");
        p
    }

    #[test]
    fn files_and_sets_build_the_metrics_config_in_order() {
        let dir = tempfile::tempdir().expect("dir");
        let res = file(
            &dir,
            "res.yaml",
            "metrics:\n  prometheus: {}\ncache_resources: []\n",
        );
        let alone = file(
            &dir,
            "metrics.yaml",
            "prometheus:\n  use_histogram_timing: true\n",
        );
        let no_section = file(&dir, "other.yaml", "cache_resources: []\n");
        load_metrics_config(None, &[res.clone(), no_section.clone()], &[]).expect("resources");
        load_metrics_config(
            Some(&alone),
            &[res],
            &["metrics.prometheus.use_histogram_timing=false".into()],
        )
        .expect("section alone");
        let sets = ["metrics.max_series=many".to_string()];
        let err = load_metrics_config(None, &[no_section], &sets).expect_err("not a number");
        assert!(
            err.contains("metrics.max_series: expected a number"),
            "{err}"
        );
        assert!(load_metrics_config(None, &[], &["metrics.x".into()]).is_err());
    }

    #[test]
    fn unreadable_or_malformed_files_name_the_file() {
        let dir = tempfile::tempdir().expect("dir");
        let missing = dir.path().join("gone.yaml");
        let err = load_metrics_config(Some(&missing), &[], &[]).expect_err("missing");
        assert!(
            err.starts_with("failed to read '") && err.contains("gone.yaml"),
            "{err}"
        );
        let bad = file(&dir, "bad.yaml", "metrics: [\n");
        let err = load_metrics_config(None, &[bad], &[]).expect_err("bad yaml");
        assert!(err.contains("bad.yaml"), "{err}");
    }
}
