#![forbid(unsafe_code)]
//! Strict validation of owner, repository, branch, tag, and tree paths at module entry (T1, S21).
//! Inputs are validated and refused, never repaired.

use crate::error::{ShakoError, ShakoResult};
use gix_validate::path::component::Options as ComponentOptions;
use std::path::{Component, Path, PathBuf};

/// Validates an organization/owner or repository name against `^[A-Za-z0-9._-]{1,100}$`
/// without leading `.`, trailing `.git` or `.lock`, `..`, NUL, backslash, or Windows device names (T1).
pub fn validate_repo_or_owner_name(name: &str, what: &str) -> ShakoResult<()> {
    if name.is_empty() || name.len() > 100 {
        return Err(ShakoError::Unprocessable(format!(
            "invalid {what} name: length must be 1..=100"
        )));
    }
    if name.starts_with('.') || name.ends_with('.') || name.ends_with(".lock") || name.ends_with(".git") {
        return Err(ShakoError::Unprocessable(format!(
            "invalid {what} name '{name}': leading dot or .git/.lock suffix is forbidden"
        )));
    }
    if name.contains("..") {
        return Err(ShakoError::Unprocessable(format!(
            "invalid {what} name '{name}': '..' is forbidden"
        )));
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
    {
        return Err(ShakoError::Unprocessable(format!(
            "invalid {what} name '{name}': only ASCII letters, digits, '.', '_' and '-' are allowed"
        )));
    }
    if gix_validate::path::component_is_windows_device(name.as_bytes().into()) {
        return Err(ShakoError::Unprocessable(format!(
            "invalid {what} name '{name}': Windows device names are forbidden"
        )));
    }
    Ok(())
}

/// Validates a full Git reference name (e.g. `refs/heads/main`, `refs/tags/v1.0`).
pub fn validate_full_ref_name(refname: &str) -> ShakoResult<()> {
    if refname.is_empty() || refname.len() > 255 {
        return Err(ShakoError::Unprocessable("invalid reference name length".into()));
    }
    if refname != "HEAD"
        && !refname.starts_with("refs/heads/")
        && !refname.starts_with("refs/tags/")
        && !refname.starts_with("refs/pull/")
    {
        return Err(ShakoError::Unprocessable(format!(
            "reference '{refname}' must be HEAD or under refs/heads/, refs/tags/, or refs/pull/"
        )));
    }
    gix_validate::reference::name(refname.as_bytes().into()).map_err(|e| {
        ShakoError::Unprocessable(format!("invalid git reference '{refname}': {e}"))
    })?;
    for comp in refname.split('/') {
        if comp.is_empty() || comp == "." || comp == ".." || comp.eq_ignore_ascii_case(".git") {
            return Err(ShakoError::Unprocessable(format!(
                "invalid component in reference '{refname}'"
            )));
        }
    }
    Ok(())
}

/// Validates a short branch name (e.g. `main`, `workspace/air-v2`, `portal/delete-x`).
pub fn validate_branch_name(branch: &str) -> ShakoResult<()> {
    if branch.is_empty() || branch.len() > 200 || branch.starts_with('/') || branch.ends_with('/') {
        return Err(ShakoError::Unprocessable(format!(
            "invalid branch name '{branch}'"
        )));
    }
    gix_validate::reference::branch_name(branch.as_bytes().into()).map_err(|e| {
        ShakoError::Unprocessable(format!("invalid branch name '{branch}': {e}"))
    })?;
    let full = format!("refs/heads/{branch}");
    validate_full_ref_name(&full)
}

/// Validates a short tag name (e.g. `v1.0.0`).
pub fn validate_tag_name(tag: &str) -> ShakoResult<()> {
    if tag.is_empty() || tag.len() > 200 {
        return Err(ShakoError::Unprocessable(format!("invalid tag name '{tag}'")));
    }
    gix_validate::tag::name(tag.as_bytes().into()).map_err(|e| {
        ShakoError::Unprocessable(format!("invalid tag name '{tag}': {e}"))
    })?;
    Ok(())
}

/// Validates a repository-relative file path (for `contents/`, trees, and archives, T1).
pub fn validate_tree_path(path: &str) -> ShakoResult<String> {
    if path.is_empty() || path.len() > 4096 {
        return Err(ShakoError::Unprocessable(
            "file path must not be empty or exceed 4096 bytes".into(),
        ));
    }
    if path.starts_with('/') || path.ends_with('/') || path.contains('\\') || path.contains('\0') {
        return Err(ShakoError::Unprocessable(format!(
            "invalid file path '{path}': leading/trailing slash, backslash, and NUL are forbidden"
        )));
    }
    let opts = ComponentOptions {
        protect_windows: true,
        protect_hfs: true,
        protect_ntfs: true,
    };
    let parts: Vec<&str> = path.split('/').collect();
    for part in &parts {
        if part.is_empty() || *part == "." || *part == ".." {
            return Err(ShakoError::Unprocessable(format!(
                "invalid file path '{path}': empty or dot segments are forbidden"
            )));
        }
        gix_validate::path::component((*part).as_bytes().into(), None, opts).map_err(|e| {
            ShakoError::Unprocessable(format!("invalid path component '{part}': {e}"))
        })?;
    }
    Ok(parts.join("/"))
}

/// Validates a package name, version, or filename (S13, T1).
pub fn validate_package_segment(seg: &str, what: &str) -> ShakoResult<()> {
    if seg.is_empty() || seg.len() > 128 {
        return Err(ShakoError::Unprocessable(format!(
            "invalid {what}: length must be 1..=128"
        )));
    }
    if seg == "."
        || seg == ".."
        || seg.contains("..")
        || seg.contains('/')
        || seg.contains('\\')
        || seg.contains('\0')
    {
        return Err(ShakoError::Unprocessable(format!("invalid {what} '{seg}'")));
    }
    if !seg
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'+' | b':'))
    {
        return Err(ShakoError::Unprocessable(format!(
            "invalid characters in {what} '{seg}'"
        )));
    }
    Ok(())
}

/// Computes the canonical bare repository disk path from `instance_id` and `repo_id` ONLY,
/// never from user-supplied owner or repository names (S2, R§2.4, Threat T1).
pub fn repo_disk_path(
    root: &Path,
    instance_id: crate::ids::InstanceId,
    repo_id: crate::ids::RepoId,
) -> ShakoResult<PathBuf> {
    let rel = format!("{instance_id}/{repo_id}.git");
    let joined = root.join(&rel);
    for comp in Path::new(&rel).components() {
        if matches!(
            comp,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        ) {
            return Err(ShakoError::Internal("invalid repo disk path".into()));
        }
    }
    Ok(joined)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_names_are_refused() {
        for bad in [
            "",
            ".",
            "..",
            ".git",
            ".hidden",
            "foo.git",
            "foo.lock",
            "a/b",
            "a\\b",
            "a\0b",
            "../etc/passwd",
            "%2e%2e",
            "CON",
            "LPT1",
            "aux",
        ] {
            assert!(
                validate_repo_or_owner_name(bad, "repo").is_err(),
                "expected '{bad}' to be refused"
            );
        }
        assert!(validate_repo_or_owner_name("helsinki_city-bikes.1", "repo").is_ok());

        for bad_ref in [
            "refs/heads/../../x",
            "refs/heads/.hidden",
            "refs/heads/foo..bar",
            "refs/heads/foo.lock",
            "refs/heads/foo@{1}",
            "refs/heads/foo\\bar",
            "refs/other/main",
        ] {
            assert!(
                validate_full_ref_name(bad_ref).is_err(),
                "expected ref '{bad_ref}' to be refused"
            );
        }
        assert!(validate_full_ref_name("refs/heads/main").is_ok());
        assert!(validate_full_ref_name("refs/heads/workspace/air-v2").is_ok());
        assert!(validate_full_ref_name("refs/tags/v1.0.0").is_ok());

        for bad_path in [
            "",
            "/a.yaml",
            "a.yaml/",
            "../a.yaml",
            "a/../b.yaml",
            "a/.git/config",
            "a/.GIT/hooks/post-receive",
            "a\\b.yaml",
            "a\0b.yaml",
        ] {
            assert!(
                validate_tree_path(bad_path).is_err(),
                "expected tree path '{bad_path}' to be refused"
            );
        }
        assert!(validate_tree_path("projects/helsinki/spaces/air/space.yaml").is_ok());
    }
}
