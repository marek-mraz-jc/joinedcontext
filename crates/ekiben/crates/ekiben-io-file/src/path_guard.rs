use std::path::{Component, Path, PathBuf};

use ekiben_core::CoreError;

/// Validates a file path to prevent directory traversal (`..`) and sandboxes it under `root_opt` when configured.
pub fn sanitize_file_path(raw_path: &str, root_opt: Option<&Path>) -> Result<PathBuf, CoreError> {
    let p = Path::new(raw_path);
    for comp in p.components() {
        if matches!(comp, Component::ParentDir) {
            return Err(CoreError::AccessDenied {
                org: "local".to_string(),
                reason: format!("path traversal ('..') is forbidden in file path '{raw_path}'"),
            });
        }
    }

    if let Some(root) = root_opt {
        let joined = if p.is_absolute() {
            let stripped = p.strip_prefix("/").unwrap_or(p);
            root.join(stripped)
        } else {
            root.join(p)
        };
        let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        if let Ok(canon_target) = joined.canonicalize() {
            if !canon_target.starts_with(&canonical_root) {
                return Err(CoreError::AccessDenied {
                    org: "local".to_string(),
                    reason: format!(
                        "file path '{raw_path}' escapes configured root '{}'",
                        root.display()
                    ),
                });
            }
        }
        Ok(joined)
    } else {
        Ok(p.to_path_buf())
    }
}
