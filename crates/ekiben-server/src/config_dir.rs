//! `ekiben streams --config-dir <dir>`: every replica of a deployment reads one directory (a
//! ConfigMap mounted in each pod) and converges to it without a restart. Each `*.yaml`/`*.yml`
//! file is a stream (id = file stem); `quotas.yaml` maps organization ids to their quotas.
//! The directory is read once a second; a file that fails to parse is reported and the last
//! good version keeps running.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use ekiben_core::{OrgId, OrgQuotas, StreamConfig};

use crate::manager::StreamManager;

pub const QUOTAS_FILE: &str = "quotas.yaml";

pub struct ConfigDir {
    dir: PathBuf,
    streams: HashMap<String, String>,
    refused: HashMap<String, String>,
    quotas: Option<String>,
    quota_orgs: Vec<String>,
}

/// Parses `quotas.yaml`: `{<org id>: {<OrgQuotas field>: value}}`. An unknown field or a limit
/// below 1 refuses the whole file, so a typo never applies half a change.
pub fn parse_quotas_file(text: &str) -> Result<BTreeMap<String, OrgQuotas>, String> {
    let value: serde_norway::Value = serde_norway::from_str(text).map_err(|e| e.to_string())?;
    if value.is_null() {
        return Ok(BTreeMap::new());
    }
    let map = value
        .as_mapping()
        .ok_or("quotas.yaml must map organization ids to quotas")?;
    let known: Vec<String> = serde_json::to_value(OrgQuotas::default())
        .ok()
        .and_then(|v| v.as_object().map(|m| m.keys().cloned().collect()))
        .unwrap_or_default();
    let mut out = BTreeMap::new();
    for (org, q) in map {
        let org = org.as_str().ok_or("an organization id must be a string")?;
        crate::api::validate_stream_id(org)
            .map_err(|m| format!("{org}: {}", m.replace("stream id", "organization id")))?;
        if let Some(fields) = q.as_mapping() {
            for key in fields.keys() {
                let name = key.as_str().unwrap_or_default();
                if !known.iter().any(|k| k == name) {
                    return Err(format!("{org}: unknown field `{name}`"));
                }
            }
        }
        let quotas: OrgQuotas =
            serde_norway::from_value(q.clone()).map_err(|e| format!("{org}: {e}"))?;
        quotas
            .validate()
            .map_err(|field| format!("{org}: {field} must be at least 1"))?;
        out.insert(org.to_string(), quotas);
    }
    Ok(out)
}

fn yaml_files(dir: &Path) -> std::io::Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // ConfigMap mounts hold `..data` and `..<timestamp>` entries beside the keys.
        if name.starts_with('.') || !(name.ends_with(".yaml") || name.ends_with(".yml")) {
            continue;
        }
        if path.is_file() {
            out.insert(name.to_string(), std::fs::read_to_string(&path)?);
        }
    }
    Ok(out)
}

impl ConfigDir {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            streams: HashMap::new(),
            refused: HashMap::new(),
            quotas: None,
            quota_orgs: Vec::new(),
        }
    }

    /// Applies what changed since the last call; returns one message per refused file.
    pub async fn sync(&mut self, manager: &StreamManager) -> Vec<String> {
        let mut errors = Vec::new();
        let mut files = match yaml_files(&self.dir) {
            Ok(f) => f,
            Err(e) => return vec![format!("{}: {e}", self.dir.display())],
        };

        // Quotas first, so a stream added in the same change starts under its limits.
        let quotas_text = files.remove(QUOTAS_FILE);
        if quotas_text != self.quotas {
            match parse_quotas_file(quotas_text.as_deref().unwrap_or("")) {
                Ok(map) => {
                    for org in &self.quota_orgs {
                        if !map.contains_key(org) {
                            manager
                                .resources
                                .set_org_quotas(&OrgId::new(org.as_str()), OrgQuotas::default());
                        }
                    }
                    for (org, q) in &map {
                        manager
                            .resources
                            .set_org_quotas(&OrgId::new(org.as_str()), q.clone());
                    }
                    self.quota_orgs = map.into_keys().collect();
                    self.quotas = quotas_text;
                }
                Err(e) => {
                    errors.push(format!("{QUOTAS_FILE}: {e}; the previous quotas stay"));
                    self.quotas = quotas_text;
                }
            }
        }

        let mut wanted = HashMap::new();
        for (name, text) in files {
            let id = Path::new(&name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            wanted.insert(id, text);
        }
        let gone: Vec<String> = self
            .streams
            .keys()
            .filter(|id| !wanted.contains_key(*id))
            .cloned()
            .collect();
        for id in gone {
            // A stream lives in the organization its file names.
            let org = self
                .streams
                .get(&id)
                .map(|t| StreamConfig::peek_org(t))
                .unwrap_or_default();
            if let Err(e) = manager.delete(&id, Some(&org)).await {
                errors.push(format!("{id}: delete failed: {e}"));
            }
            self.streams.remove(&id);
        }
        self.refused.retain(|id, _| wanted.contains_key(id));
        for (id, text) in wanted {
            if self.streams.get(&id) == Some(&text) || self.refused.get(&id) == Some(&text) {
                continue;
            }
            // `${VAR}` comes from the file's organization's variables and the allow-listed
            // process variables (E1), never the whole environment.
            let posted = StreamConfig::posted_json(&text);
            let previous_org = self.streams.get(&id).map(|t| StreamConfig::peek_org(t));
            let applied = match manager.parse_config(&text, false) {
                Err(e) => Err(e.to_string()),
                Ok((conf, _)) if previous_org.as_ref() == Some(&conf.org) => {
                    let org = conf.org.clone();
                    manager
                        .update_posted(&id, conf, Some(&org), posted)
                        .await
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                }
                Ok((conf, _)) => {
                    if let Some(old_org) = &previous_org {
                        let _ = manager.delete(&id, Some(old_org)).await;
                    }
                    manager
                        .create_posted(id.clone(), conf, posted)
                        .await
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                }
            };
            match applied {
                Ok(()) => {
                    self.refused.remove(&id);
                    self.streams.insert(id, text);
                }
                Err(e) => {
                    errors.push(format!("{id}: {e}; the previous version keeps running"));
                    // Remembered so a broken file is reported once, not every second.
                    self.refused.insert(id, text);
                }
            }
        }
        errors
    }

    /// Syncs once, then once a second forever.
    pub async fn watch(mut self, manager: StreamManager) {
        loop {
            for e in self.sync(&manager).await {
                tracing::error!(dir = %self.dir.display(), "config dir: {e}");
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
}
