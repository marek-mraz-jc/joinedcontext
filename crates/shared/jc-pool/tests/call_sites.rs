//! Every sqlx pool jc opens outside a test starts at `jc_pool::options` (T-7499): one opened
//! elsewhere keeps a cut-off connection's slot for good.
//!
//! ponytail: reads source text and stops at a file's test module; a lint of resolved paths if a
//! pool ever slips past it.

use std::path::Path;

const STARTS: [&str; 7] = [
    "PoolOptions::new(",
    "PoolOptions::<",
    "PoolOptions>::new(",
    "$opts>::new(",
    "Pool::connect",
    "PgListener::connect(",
    "PoolOptions::default(",
];

fn visit(root: &Path, dir: &Path, seen: &mut usize, found: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).expect("a readable directory") {
        let path = entry.expect("an entry").path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if path.is_dir() {
            let skipped = [
                "tests",
                "examples",
                "benches",
                "target",
                "node_modules",
                "jc-pool",
            ];
            if !skipped.contains(&name) && !name.starts_with('.') && !path.is_symlink() {
                visit(root, &path, seen, found);
            }
        } else if name.ends_with(".rs") {
            *seen += 1;
            let text = std::fs::read_to_string(&path).expect("a readable source");
            let shown = path.strip_prefix(root).expect("under the root").display();
            let lines: Vec<&str> = text.lines().map(str::trim).collect();
            for (at, line) in lines.iter().enumerate() {
                let test_module = *line == "#[cfg(test)]"
                    && lines[at + 1..]
                        .iter()
                        .find(|next| !next.starts_with("#[") && !next.starts_with("//"))
                        .is_some_and(|next| next.contains("mod ") && next.ends_with('{'));
                if test_module {
                    break;
                }
                if !line.starts_with("//") && STARTS.iter().any(|start| line.contains(start)) {
                    found.push(format!("{shown}:{}: {line}", at + 1));
                }
            }
        }
    }
}

#[test]
fn every_pool_outside_a_test_starts_at_the_shared_options() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let (mut seen, mut found) = (0, Vec::new());
    for dir in ["bins", "crates", "xtask", "vendor/joinedcontext-platform"] {
        visit(&root, &root.join(dir), &mut seen, &mut found);
    }
    assert!(seen > 1000, "only {seen} sources were read");
    assert!(
        found.is_empty(),
        "start these at jc_pool::options():\n{}",
        found.join("\n")
    );
}
