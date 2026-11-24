//! `jcctl fmt [--check] PATH...` (T-5052): writes manifest YAML in the one canonical layout of
//! [`jc_core::yaml_fmt`], the layout the Portal commits in.
//!
//! A path is a file or a directory walked for `.yaml`, `.yml` and `.md` files. In a Markdown
//! page only the ```` ```yaml ```` blocks that hold a manifest (an `apiVersion` and a `kind` at
//! the top) are formatted; the prose stays as it is. `--check` writes nothing and lists the files
//! whose layout differs.

use std::path::{Path, PathBuf};

use jc_core::yaml_fmt::{self, FmtError};

/// What one run found: the files it rewrote (or, with `--check`, would rewrite) and the files
/// it could not format, with the reason.
#[derive(Debug, Default, PartialEq)]
pub struct Report {
    /// Files whose layout differs from the canonical one.
    pub changed: Vec<PathBuf>,
    /// Files the formatter refused, with the reason.
    pub failed: Vec<(PathBuf, String)>,
}

/// Formats every YAML and Markdown file under `paths`; writes them back unless `check`.
pub fn run(paths: &[PathBuf], check: bool) -> std::io::Result<Report> {
    let mut report = Report::default();
    let mut found = Vec::new();
    for path in paths {
        found.extend(files(path)?);
    }
    for file in found {
        let src = std::fs::read_to_string(&file)?;
        match format_file(&file, &src) {
            Ok(out) if out == src => {}
            Ok(out) => {
                if !check {
                    std::fs::write(&file, out)?;
                }
                report.changed.push(file);
            }
            Err(e) => report.failed.push((file, e.to_string())),
        }
    }
    Ok(report)
}

/// The YAML and Markdown files under `path`, in a stable order. A path that is not there or a
/// directory that does not read is an error: a mistyped path must not pass `--check` unread.
fn files(path: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(path) {
        let entry = entry?;
        let wanted = matches!(
            entry.path().extension().and_then(|e| e.to_str()),
            Some("yaml" | "yml" | "md")
        );
        if wanted && entry.file_type().is_file() {
            out.push(entry.into_path());
        }
    }
    out.sort();
    Ok(out)
}

/// One file's text, formatted.
pub fn format_file(path: &Path, src: &str) -> Result<String, FmtError> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("md") => format_markdown(src),
        _ => yaml_fmt::format(src),
    }
}

/// A Markdown page with each manifest block formatted; other blocks and the prose untouched.
pub fn format_markdown(src: &str) -> Result<String, FmtError> {
    let mut out = String::with_capacity(src.len());
    let mut block: Option<Vec<&str>> = None;
    for line in src.split_inclusive('\n') {
        let trimmed = line.trim_end();
        match block.as_mut() {
            None => {
                out.push_str(line);
                if trimmed == "```yaml" {
                    block = Some(Vec::new());
                }
            }
            Some(lines) if trimmed == "```" => {
                out.push_str(&format_snippet(&lines.concat())?);
                out.push_str(line);
                block = None;
            }
            Some(lines) => lines.push(line),
        }
    }
    if let Some(lines) = block {
        out.push_str(&lines.concat());
    }
    Ok(out)
}

/// A fenced block: formatted when it is a whole manifest, as written otherwise.
fn format_snippet(text: &str) -> Result<String, FmtError> {
    let manifest = text.lines().any(|l| l.starts_with("apiVersion:"))
        && text.lines().any(|l| l.starts_with("kind:"));
    if !manifest {
        return Ok(text.to_string());
    }
    yaml_fmt::format(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_formats_manifest_blocks_and_leaves_the_rest() {
        let src = "# Title\n\n```yaml\nkind: K\napiVersion: v1\n```\n\nText.\n\n```yaml\na:   [1]\n```\n\n```json\n{}\n```\n";
        let want = "# Title\n\n```yaml\napiVersion: v1\nkind: K\n```\n\nText.\n\n```yaml\na:   [1]\n```\n\n```json\n{}\n```\n";
        assert_eq!(format_markdown(src), Ok(want.to_string()));
    }

    #[test]
    fn a_block_with_a_kind_but_no_api_version_or_the_other_way_is_left_as_written() {
        for block in ["a:   [1]\nkind: K\n", "a:   [1]\napiVersion: v1\n"] {
            let src = format!("```yaml\n{block}```\n");
            assert_eq!(format_markdown(&src), Ok(src.clone()), "{block}");
        }
    }

    #[test]
    fn check_lists_files_to_change_and_writes_nothing_and_a_run_writes_them() {
        let dir = std::env::temp_dir().join(format!("jcctl-fmt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).expect("temp dir");
        let messy = dir.join("sub/a.yaml");
        let tidy = dir.join("b.yml");
        let broken = dir.join("c.yaml");
        std::fs::write(&messy, "kind: K\napiVersion: v1\n").expect("write");
        std::fs::write(&tidy, "apiVersion: v1\nkind: K\n").expect("write");
        std::fs::write(&broken, "a: &x 1\n").expect("write");
        let check = run(std::slice::from_ref(&dir), true).expect("check");
        assert_eq!(check.changed, vec![messy.clone()]);
        assert_eq!(check.failed.len(), 1);
        assert_eq!(
            std::fs::read_to_string(&messy).ok().as_deref(),
            Some("kind: K\napiVersion: v1\n")
        );
        let write = run(std::slice::from_ref(&dir), false).expect("write run");
        assert_eq!(write.changed, vec![messy.clone()]);
        assert_eq!(
            std::fs::read_to_string(&messy).ok().as_deref(),
            Some("apiVersion: v1\nkind: K\n")
        );
        assert!(run(std::slice::from_ref(&dir), true)
            .expect("again")
            .changed
            .is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_path_that_is_not_there_is_an_error_naming_it_and_formats_nothing() {
        let dir = std::env::temp_dir().join(format!("jcctl-fmt-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let messy = dir.join("a.yaml");
        std::fs::write(&messy, "kind: K\napiVersion: v1\n").expect("write");
        let missing = dir.join("typo");
        let err = run(&[missing.clone(), dir.clone()], false).expect_err("typo is not there");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        assert!(err.to_string().contains("typo"), "{err}");
        assert_eq!(
            std::fs::read_to_string(&messy).ok().as_deref(),
            Some("kind: K\napiVersion: v1\n"),
            "nothing is written when one path does not read"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unclosed_markdown_block_stays_as_written_and_other_extensions_are_left() {
        let dir = std::env::temp_dir().join(format!("jcctl-fmt-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let page = dir.join("page.md");
        let other = dir.join("notes.txt");
        std::fs::write(&page, "```yaml\nkind: K\napiVersion: v1\n").expect("write");
        std::fs::write(&other, "kind: K\napiVersion: v1\n").expect("write");
        let report = run(&[page.clone(), other.clone()], true).expect("check");
        assert!(
            report.changed.is_empty() && report.failed.is_empty(),
            "an unclosed block stays as written and a .txt is no manifest: {report:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
