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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_without_a_root_stays_and_a_parent_step_is_refused() {
        assert_eq!(
            sanitize_file_path("/data/a.txt", None).expect("kept"),
            PathBuf::from("/data/a.txt")
        );
        let e = sanitize_file_path("data/../../etc/passwd", None).expect_err("refused");
        assert!(matches!(e, CoreError::AccessDenied { .. }), "{e}");
    }

    #[test]
    fn a_root_takes_absolute_and_relative_paths_under_it_and_refuses_a_symlink_out() {
        let root = tempfile::TempDir::new().expect("root");
        let outside = tempfile::TempDir::new().expect("outside");
        let r = root.path();
        assert_eq!(
            sanitize_file_path("/a/b.txt", Some(r)).expect("abs"),
            r.join("a/b.txt")
        );
        assert_eq!(
            sanitize_file_path("a/b.txt", Some(r)).expect("rel"),
            r.join("a/b.txt")
        );
        std::os::unix::fs::symlink(outside.path(), r.join("out")).expect("symlink");
        let e = sanitize_file_path("out", Some(r)).expect_err("escapes");
        assert!(e.to_string().contains("escapes configured root"), "{e}");
        // a link that stays inside the root is fine
        std::fs::create_dir(r.join("in")).expect("dir");
        std::os::unix::fs::symlink(r.join("in"), r.join("link")).expect("symlink");
        assert_eq!(
            sanitize_file_path("link", Some(r)).expect("inside"),
            r.join("link")
        );
        // a root that does not exist yet is taken as written
        let later = r.join("later");
        assert_eq!(
            sanitize_file_path("x", Some(&later)).expect("kept"),
            later.join("x")
        );
    }
}
