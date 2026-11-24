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
            let (Some(org), Some(ext)) = (
                path.file_stem().and_then(|s| s.to_str()),
                path.extension().and_then(|e| e.to_str()),
            ) else {
                continue;
            };
            if ext != "yaml" && ext != "yml" {
                continue;
            }
            let org = org.to_string();
            if !valid_org(&org) {
                errors.push(format!("{}: not a valid organization id", path.display()));
                continue;
            }
            // The target's size, not a symlink's own: a link to a huge file is refused too.
            match std::fs::metadata(&path) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(resources: &ResourceManager, org: &str) -> HashMap<String, String> {
        resources
            .get_or_create_org(&OrgId::new(org))
            .secrets_snapshot()
    }

    #[test]
    fn a_file_parses_to_strings_and_names_only_the_problem() {
        assert!(parse_org_vars("  \n").expect("blank").is_empty());
        assert!(parse_org_vars("~").expect("null").is_empty());
        let v = parse_org_vars("HOST: db\nPORT: 5432\nTLS: true\n_X1: ''\n").expect("map");
        assert_eq!(v["HOST"], "db");
        assert_eq!(v["PORT"], "5432");
        assert_eq!(v["TLS"], "true");
        assert_eq!(v["_X1"], "");
        for bad in ["- a\n- b\n", "plain", "a: [\n"] {
            assert_eq!(
                parse_org_vars(bad).expect_err(bad),
                "not a YAML map of NAME: value"
            );
        }
        let err = parse_org_vars("1ST: secret-value\n").expect_err("name");
        assert!(
            err.contains("'1ST'") && !err.contains("secret-value"),
            "{err}"
        );
        assert!(parse_org_vars("A-B: x\n").is_err());
        let err = parse_org_vars("NESTED: {a: hidden}\n").expect_err("nested");
        assert!(err.contains("NESTED") && !err.contains("hidden"), "{err}");
        assert!(parse_org_vars("NULLED: ~\n").is_err());
    }

    #[test]
    fn more_than_the_variable_limit_is_refused() {
        let text: String = (0..=MAX_VARS_PER_ORG)
            .map(|i| format!("V{i}: x\n"))
            .collect();
        let err = parse_org_vars(&text).expect_err("too many");
        assert!(err.contains("more than 4096"), "{err}");
    }

    #[test]
    fn organization_ids_follow_the_stream_id_alphabet() {
        assert!(valid_org("org-a.b_c:1"));
        assert!(!valid_org(""));
        assert!(!valid_org("a b"));
        assert!(!valid_org(&"o".repeat(257)));
        assert!(valid_org(&"o".repeat(256)));
    }

    #[test]
    fn sync_applies_changes_keeps_broken_files_and_empties_removed_ones() {
        let dir = tempfile::tempdir().expect("dir");
        let p = dir.path();
        let resources = ResourceManager::new();
        let mut watcher = OrgVarsDir::new(p);
        std::fs::write(p.join("org-a.yaml"), "HOST: a\n").expect("write");
        std::fs::write(p.join("org-b.yml"), "HOST: b\n").expect("write");
        std::fs::write(p.join("notes.txt"), "HOST: c\n").expect("write");
        std::fs::write(p.join("README"), "no extension").expect("write");
        assert!(watcher.sync(&resources).is_empty());
        assert_eq!(vars(&resources, "org-a")["HOST"], "a");
        assert_eq!(vars(&resources, "org-b")["HOST"], "b");

        // Unchanged: nothing to do. Broken: reported by name, the previous map stays.
        assert!(watcher.sync(&resources).is_empty());
        std::fs::write(p.join("org-a.yaml"), "HOST: [\n").expect("write");
        let errors = watcher.sync(&resources);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].starts_with("org-a.yaml: not a YAML map"),
            "{errors:?}"
        );
        assert_eq!(vars(&resources, "org-a")["HOST"], "a");

        std::fs::remove_file(p.join("org-b.yml")).expect("rm");
        std::fs::write(p.join("org-a.yaml"), "HOST: a2\n").expect("write");
        assert!(watcher.sync(&resources).is_empty());
        assert!(vars(&resources, "org-b").is_empty());
        assert_eq!(vars(&resources, "org-a")["HOST"], "a2");
    }

    #[test]
    fn sync_reports_bad_names_unreadable_and_oversized_files() {
        let dir = tempfile::tempdir().expect("dir");
        let p = dir.path();
        let resources = ResourceManager::new();
        let mut watcher = OrgVarsDir::new(p);
        std::fs::write(p.join("org-big.yaml"), "HOST: small\n").expect("write");
        assert!(watcher.sync(&resources).is_empty());

        std::fs::write(p.join("bad org.yaml"), "HOST: x\n").expect("write");
        std::fs::create_dir(p.join("org-dir.yaml")).expect("dir");
        let huge = vec![b'#'; MAX_FILE_BYTES as usize + 1];
        let target = p.join("huge.bin");
        std::fs::write(&target, &huge).expect("write");
        std::fs::remove_file(p.join("org-big.yaml")).expect("rm");
        std::os::unix::fs::symlink(&target, p.join("org-big.yaml")).expect("link");
        std::os::unix::fs::symlink(p.join("nowhere"), p.join("org-gone.yaml")).expect("link");
        std::os::unix::fs::symlink(&target, p.join("org-new.yaml")).expect("link");
        let mut errors = watcher.sync(&resources);
        errors.sort();
        assert_eq!(errors.len(), 5, "{errors:?}");
        assert!(vars(&resources, "org-new").is_empty());
        assert!(errors
            .iter()
            .any(|e| e.contains("bad org.yaml: not a valid organization id")));
        assert!(errors
            .iter()
            .any(|e| e.contains("org-big.yaml: larger than 1048576 bytes")));
        assert!(errors.iter().any(|e| e.contains("org-dir.yaml:")));
        assert!(errors.iter().any(|e| e.contains("org-gone.yaml:")));
        // An oversized file keeps the organization's previous variables.
        assert_eq!(vars(&resources, "org-big")["HOST"], "small");

        let errors = OrgVarsDir::new(p.join("missing")).sync(&resources);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("missing"), "{errors:?}");
    }

    #[tokio::test]
    async fn watch_reads_the_directory_again_every_second() {
        let dir = tempfile::tempdir().expect("dir");
        let resources = ResourceManager::new();
        tokio::spawn(OrgVarsDir::new(dir.path()).watch(resources.clone()));
        std::fs::write(dir.path().join("org-w.yaml"), "HOST: w\n").expect("write");
        std::fs::write(dir.path().join("org-x.yaml"), "HOST: [\n").expect("write");
        for _ in 0..50 {
            if vars(&resources, "org-w").contains_key("HOST") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert_eq!(vars(&resources, "org-w")["HOST"], "w");
    }
}
