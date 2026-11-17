//! ARCH/TEN-10: an internal space, Tenant, subject or object name is built by
//! `jc_types::internal_name` (or `ProjectKey::subject` / `object_prefix`), never from text.
//! This test scans every `.rs` file of the monorepo (outside `tests/` directories) for the
//! constructors that build such a name from a project or space name and fails on any call site
//! beyond the baseline of 2026-11-17 (T-7190). A baseline row is a known deviation that a
//! carrier task of group project-key moves to the builder; lower its count when you do.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Text-built name constructors.
const PATTERNS: &[&str] = &[
    "Tenant::segment(",
    "TenantName::new(",
    "format!(\"ws-",
    "format!(\"{project}-",
    "format!(\"org/",
    "format!(\"jc.",
];

/// (file relative to the monorepo root, pattern, call sites) on 2026-11-17.
const BASELINE: &[(&str, &str, usize)] = &[
    (
        "vendor/joinedcontext-platform/crates/context-gateway/benches/pdp_decision.rs",
        "Tenant::segment(",
        1,
    ),
    (
        "vendor/joinedcontext-platform/crates/context-gateway/src/fanout/hub.rs",
        "Tenant::segment(",
        1,
    ),
    (
        "vendor/joinedcontext-platform/crates/context-gateway/src/fanout/socket.rs",
        "Tenant::segment(",
        7,
    ),
    (
        "vendor/joinedcontext-platform/crates/context-gateway/src/handlers/schema.rs",
        "Tenant::segment(",
        1,
    ),
    (
        "vendor/joinedcontext-platform/crates/context-gateway/src/mcp/catalog.rs",
        "Tenant::segment(",
        1,
    ),
    (
        "vendor/joinedcontext-platform/crates/context-gateway/src/mcp/hub.rs",
        "Tenant::segment(",
        1,
    ),
    (
        "vendor/joinedcontext-platform/crates/context-gateway/src/middleware/tenancy.rs",
        "Tenant::segment(",
        3,
    ),
    (
        "vendor/joinedcontext-platform/crates/context-gateway/src/store.rs",
        "Tenant::segment(",
        1,
    ),
    (
        "vendor/joinedcontext-platform/crates/context-gateway/src/store.rs",
        "TenantName::new(",
        1,
    ),
    (
        "vendor/joinedcontext-platform/crates/jc-core/src/names.rs",
        "TenantName::new(",
        3,
    ),
    ("crates/portal/src/ops/previews.rs", "format!(\"ws-", 2),
    (
        "crates/portal/src/api/access_matrix.rs",
        "format!(\"{project}-",
        1,
    ),
    (
        "crates/portal/src/api/access_simulate.rs",
        "format!(\"{project}-",
        1,
    ),
    (
        "crates/portal/src/api/connectors.rs",
        "format!(\"{project}-",
        1,
    ),
    ("crates/portal/src/api/export.rs", "format!(\"{project}-", 4),
    (
        "crates/portal/src/api/mqtt_test_publish.rs",
        "format!(\"{project}-",
        1,
    ),
    (
        "crates/portal/src/ops/compute.rs",
        "format!(\"{project}-",
        2,
    ),
    (
        "crates/portal/src/personal_connectors.rs",
        "format!(\"{project}-",
        1,
    ),
    (
        "vendor/joinedcontext-platform/crates/jc-core/src/kinds/service_account.rs",
        "format!(\"{project}-",
        1,
    ),
    (
        "vendor/joinedcontext-platform/crates/jc-core/src/kinds/space.rs",
        "format!(\"{project}-",
        1,
    ),
    (
        "vendor/joinedcontext-platform/crates/jc-core/src/reach.rs",
        "format!(\"{project}-",
        1,
    ),
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if !matches!(
                name.as_ref(),
                "target" | "node_modules" | ".git" | "tests" | ".cg"
            ) && !name.starts_with("target")
            {
                walk(&path, out);
            }
        } else if name.ends_with(".rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_new_internal_name_is_built_from_text() {
    let root = root();
    let mut files = Vec::new();
    for top in ["crates", "bins", "vendor"] {
        walk(&root.join(top), &mut files);
    }
    assert!(
        files.len() > 100,
        "the scan found {} files under {}",
        files.len(),
        root.display()
    );
    let allowed: BTreeMap<(&str, &str), usize> =
        BASELINE.iter().map(|(f, p, n)| ((*f, *p), *n)).collect();
    let mut over = Vec::new();
    for path in files {
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if rel.starts_with("crates/shared/jc-types/") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        for pattern in PATTERNS {
            let found = text.matches(pattern).count();
            let limit = allowed.get(&(rel.as_str(), *pattern)).copied().unwrap_or(0);
            if found > limit {
                over.push(format!("{rel}: {found} x {pattern} (baseline {limit})"));
            }
        }
    }
    assert!(
        over.is_empty(),
        "internal names built from text (ARCH/TEN-10); build them with jc_types::internal_name, ProjectKey::subject or ProjectKey::object_prefix:\n{}",
        over.join("\n")
    );
}
