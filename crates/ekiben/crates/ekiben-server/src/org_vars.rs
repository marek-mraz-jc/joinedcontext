//! `--org-vars <dir>`: each organization's variables, one `<org>.yaml` (or `.yml`) map of
//! `NAME: value` per organization. They fill `${NAME}` in that organization's configs and answer
//! Bloblang `env("NAME")` in its streams, before any `EKIBEN_VAR_*` process variable (E1). The
//! directory is read once a second; a changed file replaces the organization's whole map at once
//! (streams read it when they start); a removed file empties it; a file that does not parse is
//! reported (by name, never by content) and the previous map stays.

use std::collections::HashMap;
use std::path::PathBuf;

use ekiben_core::{OrgId, ResourceManager};
use serde_json::Value as JsonValue;

const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_VARS_PER_ORG: usize = 4096;

/// Watches a directory of organization variable files.
pub struct OrgVarsDir {
    dir: PathBuf,
    seen: HashMap<String, String>,
}

fn valid_org(org: &str) -> bool {
    !org.is_empty()
        && org.len() <= 256
        && org
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':'))
}

fn valid_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Parses one organization's file; an error names the variable or the problem, never a value.
pub fn parse_org_vars(text: &str) -> Result<HashMap<String, String>, String> {
    if text.trim().is_empty() {
        return Ok(HashMap::new());
    }
    let value: JsonValue =
        serde_norway::from_str(text).map_err(|_| "not a YAML map of NAME: value".to_string())?;
    let map = match value {
        JsonValue::Object(m) => m,
        JsonValue::Null => return Ok(HashMap::new()),
        _ => return Err("not a YAML map of NAME: value".to_string()),
    };
    if map.len() > MAX_VARS_PER_ORG {
        return Err(format!("more than {MAX_VARS_PER_ORG} variables"));
    }
    let mut out = HashMap::with_capacity(map.len());
    for (k, v) in map {
        if !valid_var_name(&k) {
            return Err(format!("variable name '{k}' is not [A-Za-z_][A-Za-z0-9_]*"));
        }
        let s = match v {
            JsonValue::String(s) => s,
            JsonValue::Number(n) => n.to_string(),
            JsonValue::Bool(b) => b.to_string(),
            _ => return Err(format!("variable {k} must be a string, number or boolean")),
        };
        out.insert(k, s);
    }
    Ok(out)
}

impl OrgVarsDir {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            seen: HashMap::new(),
        }
    }

    /// Applies the directory to the organizations once; returns the problems found.
    pub fn sync(&mut self, resources: &ResourceManager) -> Vec<String> {
        let mut errors = Vec::new();
        let mut present: HashMap<String, String> = HashMap::new();
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) => {
                errors.push(format!("{}: {e}", self.dir.display()));
                return errors;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default();
            if ext != "yaml" && ext != "yml" {
                continue;
            }
            let Some(org) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            if !valid_org(&org) {
                errors.push(format!("{}: not a valid organization id", path.display()));
                continue;
            }
            match entry.metadata() {
                Ok(m) if m.len() > MAX_FILE_BYTES => {
                    errors.push(format!(
                        "{}: larger than {MAX_FILE_BYTES} bytes",
                        path.display()
                    ));
                    if let Some(old) = self.seen.get(&org) {
                        present.insert(org, old.clone());
                    }
                    continue;
                }
                Ok(_) => {}
                Err(e) => {
                    errors.push(format!("{}: {e}", path.display()));
                    continue;
                }
            }
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    present.insert(org, text);
                }
                Err(e) => errors.push(format!("{}: {e}", path.display())),
            }
        }

        for (org, text) in &present {
            if self.seen.get(org) == Some(text) {
                continue;
            }
            match parse_org_vars(text) {
                Ok(vars) => {
                    resources
                        .get_or_create_org(&OrgId::new(org.as_str()))
                        .replace_secrets(vars);
                    self.seen.insert(org.clone(), text.clone());
                }
                Err(e) => {
                    errors.push(format!("{org}.yaml: {e}; the previous variables stay"));
                }
            }
        }
        let gone: Vec<String> = self
            .seen
            .keys()
            .filter(|org| !present.contains_key(*org))
            .cloned()
            .collect();
        for org in gone {
            resources
                .get_or_create_org(&OrgId::new(org.as_str()))
                .replace_secrets(HashMap::new());
            self.seen.remove(&org);
        }
        errors
    }

    /// Syncs once a second forever.
    pub async fn watch(mut self, resources: ResourceManager) {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            for e in self.sync(&resources) {
                tracing::error!(dir = %self.dir.display(), "org vars: {e}");
            }
        }
    }
}
