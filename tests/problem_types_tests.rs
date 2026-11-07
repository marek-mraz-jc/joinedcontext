//! Every problem type the Portal API answers with has words in the Portal (T-3243, API/00 §4):
//! the hint `problem.{slug}` the UI shows beside the API's sentence, in every locale it ships.
//! The source walk is the audit: a slug the Portal starts answering with and nobody translated
//! fails here, and so does a fourth place that builds a problem's `type` by hand.

use std::path::{Path, PathBuf};

fn sources(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("a readable source directory") {
        let path = entry.expect("an entry").path();
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
}

fn source_files() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    sources(&root.join("src"), &mut files);
    files
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path).expect("a readable source file");
            let shown = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string();
            (shown, text)
        })
        .collect()
}

/// The three places that build a problem's `type`, and the slugs they are given: the `ApiError`
/// mapping of `error.rs` (a `StatusCode::…` line, then the slug), basemap's `problem_response` and
/// the App services' `problem`, called in `apps/services.rs` and `apps/ai.rs` (API/06, T-3699).
const AFTER_STATUS: [&str; 4] = ["error.rs", "basemap.rs", "apps/services.rs", "apps/ai.rs"];

fn slugs() -> Vec<(String, String)> {
    let mut found = Vec::new();
    for (file, text) in source_files() {
        let lines: Vec<&str> = text.lines().collect();
        for (at, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            let after_status = at > 0 && lines[at - 1].trim().starts_with("StatusCode::");
            let one_line = trimmed.contains("(StatusCode::") && trimmed.contains("\", \"");
            if file.ends_with("error.rs") && one_line {
                if let Some(slug) = trimmed.split('"').nth(1) {
                    found.push((file.clone(), slug.to_owned()));
                }
            } else if AFTER_STATUS.iter().any(|f| file.ends_with(f)) && after_status {
                if let Some(slug) = trimmed
                    .strip_prefix('"')
                    .and_then(|s| s.strip_suffix("\","))
                {
                    found.push((file.clone(), slug.to_owned()));
                }
            }
        }
    }
    found
}

#[test]
fn every_problem_type_of_the_portal_has_its_hint_in_every_locale() {
    let found = slugs();
    assert!(found.len() > 15, "the walk found the slugs: {found:?}");
    let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/src/locales");
    for locale in ["en", "sk", "cs", "de"] {
        let text = std::fs::read_to_string(ui.join(format!("{locale}.json"))).expect("a locale");
        let catalogue: serde_json::Value = serde_json::from_str(&text).expect("a json locale");
        let missing: Vec<String> = found
            .iter()
            .filter(|(_, slug)| !catalogue["problem"][slug].is_string())
            .map(|(file, slug)| format!("{file}: {slug}"))
            .collect();
        assert!(
            missing.is_empty(),
            "{locale}: a problem type with no `problem.<slug>` hint (add it to every locale, or use a type of the catalogue in API/00 §4):\n  {}",
            missing.join("\n  ")
        );
    }
}

#[test]
fn a_problem_type_is_built_in_the_three_known_places_only() {
    let mut builders: Vec<String> = source_files()
        .into_iter()
        .filter(|(_, text)| text.contains("joinedcontext.com/errors/{"))
        .map(|(file, _)| file)
        .collect();
    builders.sort();
    assert_eq!(
        builders,
        vec!["src/api/basemap.rs", "src/apps/services.rs", "src/error.rs"],
        "a new place builds a problem's type by hand; route it through ApiError, or add it to this walk"
    );
}
