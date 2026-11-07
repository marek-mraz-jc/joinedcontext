//! Every environment variable the Portal reads is explained where it is read (T-2140, OPS-27).
//!
//! `docs/Deployment/13-configuration-reference.md` is generated from these doc comments: the
//! sentence an operator reads about `JC_PORTAL_DATABASE_URL` is the rustdoc of the field that
//! reads it. A variable added without one leaves a bare name in the reference, and the docs
//! lane only notices after this repository is merged — this notices here, before it is.
//!
//! A source scan, reading `src/` the way `docs/scripts/generate-config-reference.py` does, so
//! the two cannot disagree about what counts as a read.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Names the Portal writes into a workload rather than reads: a builder run's workspace and a
/// generated application. They are documented where they are written, and listed separately in
/// the reference, because they are a contract with an application rather than a setting.
const INJECTED: &[&str] = &["JC_ORG_DOMAIN", "JC_SOURCE_SPACE", "JC_SPACE"];

fn source_root() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    assert!(
        root.join("config.rs").is_file(),
        "{} does not hold config.rs: the layout moved and this scan is looking in the wrong \
         place, which would make it pass by finding nothing",
        root.display(),
    );
    root
}

fn sources(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(sources(&path));
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
    found.sort();
    found
}

/// The source up to its test module: a test sets variables rather than reading them.
fn without_tests(text: &str) -> &str {
    match text.find("#[cfg(test)]") {
        Some(at) => &text[..at],
        None => text,
    }
}

/// Every `JC_…`/`PORTAL_…` name on a line, with whether it is written as a string literal.
fn names_on(line: &str) -> Vec<(String, bool)> {
    let bytes = line.as_bytes();
    let mut found = Vec::new();
    let mut at = 0;
    while at < line.len() {
        let rest = &line[at..];
        let Some(start) = ["JC_", "PORTAL_"]
            .iter()
            .filter_map(|prefix| rest.find(prefix))
            .min()
        else {
            break;
        };
        let begin = at + start;
        let end = line[begin..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .map_or(line.len(), |offset| begin + offset);
        let name = &line[begin..end];
        if !name.ends_with('_') && name.len() > 3 {
            let quoted =
                begin > 0 && bytes[begin - 1] == b'"' && end < bytes.len() && bytes[end] == b'"';
            found.push((name.to_owned(), quoted));
        }
        at = end.max(begin + 1);
    }
    found
}

fn is_doc(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("///") || trimmed.starts_with("//!")
}

/// What the Portal reads, and everything it documents.
fn read_and_documented() -> (BTreeMap<String, BTreeSet<String>>, BTreeSet<String>) {
    let mut read: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut documented = BTreeSet::new();
    for path in sources(&source_root()) {
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let file = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        for line in without_tests(&text).lines() {
            if is_doc(line) {
                for (name, _) in names_on(line) {
                    documented.insert(name);
                }
                continue;
            }
            if line.trim_start().starts_with("//") {
                continue;
            }
            // `{ "name": "JC_X", "value": … }` writes the variable into a workload.
            let writes = line.contains("\"name\"");
            for (name, quoted) in names_on(line) {
                if quoted && !writes && !INJECTED.contains(&name.as_str()) {
                    read.entry(name).or_default().insert(file.clone());
                }
            }
        }
    }
    (read, documented)
}

/// OPS-27: a variable the Portal reads and nothing explains is a name the configuration
/// reference would print without a sentence, and a setting an operator has to guess at.
#[test]
fn every_variable_the_portal_reads_is_documented_where_it_is_read() {
    let (read, documented) = read_and_documented();
    assert!(
        read.len() > 40,
        "only {} variable(s) found: the scan is looking in the wrong place, which would make \
         this test pass by finding nothing",
        read.len(),
    );

    let mut undocumented = Vec::new();
    for (name, files) in &read {
        if !documented.contains(name) {
            let where_read = files.iter().cloned().collect::<Vec<_>>().join(", ");
            undocumented.push(format!("{name} (read in {where_read})"));
        }
    }
    assert!(
        undocumented.is_empty(),
        "{} variable(s) are read and no doc comment says what they are:\n  {}\n\
         Document each one where it is read: the configuration reference is generated from \
         those sentences.",
        undocumented.len(),
        undocumented.join("\n  "),
    );
}

/// OPS-27: the Portal holds the forge token, the cookie key and the client secret. Reading one
/// into a log line writes it into the cluster's log store, where it outlives every rotation.
#[test]
fn no_secret_is_read_straight_into_a_log_line() {
    let logs = [
        "tracing::trace!",
        "tracing::debug!",
        "tracing::info!",
        "tracing::warn!",
        "tracing::error!",
        "println!",
        "eprintln!",
        "dbg!",
    ];
    let reads = ["env::var(", "env::var_os(", "lookup(", "var(", "var_os("];

    let mut leaked = Vec::new();
    for path in sources(&source_root()) {
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        for (number, line) in without_tests(&text).lines().enumerate() {
            if is_doc(line) || !logs.iter().any(|name| line.contains(name)) {
                continue;
            }
            for (name, quoted) in names_on(line) {
                let read_here = reads.iter().any(|call| {
                    line.find(call)
                        .is_some_and(|at| line[at..].contains(&format!("\"{name}\"")))
                });
                if quoted && read_here && is_secret(&name) {
                    leaked.push(format!("{}:{}: {name}", path.display(), number + 1));
                }
            }
        }
    }
    assert!(
        leaked.is_empty(),
        "a secret's value is read into a log line:\n  {}",
        leaked.join("\n  "),
    );
}

/// What a name has to look like to hold a credential rather than an address or a path to one.
fn is_secret(name: &str) -> bool {
    if name.ends_with("_FILE") {
        return false;
    }
    if name.ends_with("_URL") {
        // The connection string carries the password inside it; every other address does not.
        return name == "JC_PORTAL_DATABASE_URL";
    }
    name.contains("SECRET") || name.contains("TOKEN") || name.ends_with("_KEY")
}

/// The two cases above are only worth anything if the reader reads what it thinks it does.
#[test]
fn the_source_reader_knows_a_read_from_a_mention() {
    assert_eq!(
        names_on(r#"    let bind = lookup("JC_PORTAL_BIND");"#),
        vec![("JC_PORTAL_BIND".to_owned(), true)],
    );
    assert_eq!(
        names_on("/// The address to listen on (`JC_PORTAL_BIND`, default)."),
        vec![("JC_PORTAL_BIND".to_owned(), false)],
    );
    assert_eq!(
        names_on(r#"{ "name": "JC_APP_NAME", "value": app }"#),
        vec![("JC_APP_NAME".to_owned(), true)],
    );
    assert!(names_on(r#".strip_prefix("JC_SPACE_")"#).is_empty());
    assert!(is_doc("    /// a field"));
    assert!(!is_doc("    // an ordinary comment"));
    assert!(is_secret("JC_GITEA_TOKEN"));
    assert!(is_secret("JC_PORTAL_COOKIE_KEY"));
    assert!(is_secret("JC_PORTAL_DATABASE_URL"), "it carries a password");
    assert!(!is_secret("JC_PORTAL_GATEWAY_URL"), "an address is not one");
    assert!(!is_secret("JC_BASEMAP_KEY_FILE"), "a path is not the value");
    assert_eq!(without_tests("a\n#[cfg(test)]\nb"), "a\n");
}
