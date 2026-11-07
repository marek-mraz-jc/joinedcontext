//! The Smart Data Models mirror (T-6353): every catalogue model converted to LinkML ahead of time
//! by `sdm_mirror.py` of Model Tools, in the image at `JC_PORTAL_SDM_MIRROR_DIR`, so the new-model
//! window answers from disk and not from GitHub per click.
//!
//! A model id only ever names a file through the index, whose ids were checked when it was read,
//! and the file must still resolve under the root. A file whose upstream commit is not the one the
//! index pins (DM-08) is a torn mirror: it is not served, and the caller asks Model Tools.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::model_tools::Catalogue;

/// The index the mirror's nightly run wrote.
const INDEX: &str = "index.json";
/// The file of one model under `<subject>/<model>/`.
const MODEL_FILE: &str = "linkml.yaml";

/// A catalogue identifier is `dataModel.<Subject>/<Model>`: two segments of the characters the
/// catalogue itself uses. Anything else — a URL, a scheme, a traversal, a query — is refused
/// before a request is built, so no caller can point the fetch at a host of their choosing.
pub(crate) fn is_catalogue_id(model: &str) -> bool {
    let Some((subject, name)) = model.split_once('/') else {
        return false;
    };
    let segment_ok = |s: &str| {
        !s.is_empty()
            && s.len() <= 128
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
            && !s.starts_with('.')
    };
    segment_ok(subject) && segment_ok(name)
}

/// What the mirror holds for one model.
#[derive(Debug, PartialEq)]
pub enum Mirrored {
    /// The LinkML source, at the commit the index pins.
    Model(String),
    /// The model did not convert upstream: why, in the converter's words.
    Failed(String),
    /// Not in the mirror, or its file is missing or at another commit: ask Model Tools.
    Unknown,
}

/// One read of the mirror's index, held for the life of the process.
#[derive(Debug)]
pub struct SdmMirror {
    root: PathBuf,
    catalogue: Catalogue,
    pins: HashMap<String, String>,
    failed: HashMap<String, String>,
}

impl SdmMirror {
    /// Reads the index under `root`. A mirror that cannot be read is an error the caller logs,
    /// and the Portal goes on through Model Tools.
    pub fn open(root: &Path) -> Result<Self, String> {
        let root = root
            .canonicalize()
            .map_err(|err| format!("{}: {err}", root.display()))?;
        let text = std::fs::read(root.join(INDEX)).map_err(|err| format!("{INDEX}: {err}"))?;
        let catalogue: Catalogue =
            serde_json::from_slice(&text).map_err(|err| format!("{INDEX}: {err}"))?;
        let mut pins = HashMap::new();
        for model in catalogue.subjects.iter().flat_map(|s| &s.models) {
            match model.commit.as_deref() {
                Some(commit) if is_catalogue_id(&model.id) => {
                    pins.insert(model.id.clone(), commit.to_owned());
                }
                _ => return Err(format!("{INDEX}: {} has no usable id or commit", model.id)),
            }
        }
        let failed = catalogue
            .failed
            .iter()
            .map(|one| (one.id.clone(), one.finding.clone()))
            .collect();
        Ok(Self {
            root,
            catalogue,
            pins,
            failed,
        })
    }

    /// The catalogue as the mirror's run listed it.
    pub fn catalogue(&self) -> &Catalogue {
        &self.catalogue
    }

    /// One model by its catalogue identifier.
    pub async fn model(&self, id: &str) -> Mirrored {
        if let Some(finding) = self.failed.get(id) {
            return Mirrored::Failed(finding.clone());
        }
        let (Some(pin), Some((subject, name))) = (self.pins.get(id), id.split_once('/')) else {
            return Mirrored::Unknown;
        };
        let path = self.root.join(subject).join(name).join(MODEL_FILE);
        // A link inside a repository of generated files has no business leaving it.
        let inside = tokio::fs::canonicalize(&path)
            .await
            .is_ok_and(|real| real.starts_with(&self.root));
        if !inside {
            return Mirrored::Unknown;
        }
        match tokio::fs::read_to_string(&path).await {
            Ok(linkml) if pinned_to(&linkml, pin) => Mirrored::Model(linkml),
            Ok(_) => {
                tracing::warn!(
                    model = id,
                    "mirrored model is not at the commit its index pins"
                );
                Mirrored::Unknown
            }
            Err(err) => {
                tracing::warn!(model = id, %err, "mirrored model could not be read");
                Mirrored::Unknown
            }
        }
    }
}

/// Whether the model's own `spec.source.commit` annotation names `pin`.
fn pinned_to(linkml: &str, pin: &str) -> bool {
    linkml.lines().any(|line| {
        line.trim_start()
            .strip_prefix("spec.source.commit:")
            .is_some_and(|commit| commit.trim().trim_matches(['\'', '"']) == pin)
    })
}
