// SPDX-License-Identifier: EUPL-1.2
//! `attr_instances` is the one tenant-bearing table that runs without a
//! row-security policy in timescale mode: TimescaleDB refuses columnstore
//! compression on a table with RLS, and ADR-0006 decided compression wins
//! there. What is left holding tenants apart on that table is the explicit
//! `tenant_id` predicate every request-path statement carries — a discipline
//! nothing enforces once the belt is off, and one statement written without
//! it reads another tenant's history with no error anywhere.
//!
//! So the discipline is read out of the source: every SQL literal in the
//! Postgres store that selects, inserts into, updates or deletes from
//! `attr_instances` either compares `tenant_id`, or is named below with the
//! reason it does not. A new statement is neither until someone says which.
//! `attr_rows` (migration 0020, T-5200), the second history parent, runs
//! without a policy in timescale mode too and is held to the same rule.
//!
//! ponytail: a small lexer over `#[cfg(test)]`-free source, not a Rust
//! parser. It reads plain and raw string literals and skips char literals
//! and comments; a table name assembled at runtime is the one shape it
//! cannot judge, so the name appearing outside a literal fails the test
//! instead of slipping past it.

use std::path::{Path, PathBuf};

/// Statements that touch the table across tenants on purpose, and why. Each
/// entry is a fragment that appears in exactly one literal.
const EXEMPT: &[(&str, &str)] = &[
    (
        "WHERE try_timestamptz(data->>'expiresAt') < now()",
        "the 4.22 expired-instance reap (both history parents): cross-tenant \
         work under the service role, which is what lets it collect every tenant's expired rows in \
         one statement",
    ),
    (
        "DELETE FROM attr_instances_default\n         WHERE observed_at < now()",
        "the ANTARES_TEMPORAL_RETENTION_DAYS purge of the DEFAULT partition: \
         a horizon is a property of the deployment, not of a tenant",
    ),
    (
        "WITH moved AS (DELETE FROM attr_instances_default",
        "partition adoption moves a whole week out of DEFAULT so the ATTACH \
         revalidates; a tenant predicate would strand the other tenants' rows \
         there and the ATTACH would fail",
    ),
    (
        "DELETE FROM attr_rows_default\n         WHERE observed_at < now()",
        "the retention purge of the second parent's DEFAULT partition",
    ),
    (
        "WITH moved AS (DELETE FROM attr_rows_default",
        "partition adoption for the second parent, as for attr_instances",
    ),
];

fn store_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/store")
}

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read store dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The string literals of `src` and the code around them, comments
/// dropped, read the way rustc reads them: `"…"` with its escapes, raw
/// strings `r"…"` and `r#"…"#`, char literals (`'"'` is a quote, not the
/// start of a string) and lifetimes. Runs over the source ahead of the first
/// `#[cfg(test)]` only: an assertion about a statement is not a statement.
fn lex(src: &str) -> (Vec<String>, String) {
    let src = match src.find("#[cfg(test)]") {
        Some(at) => &src[..at],
        None => src,
    };
    let c: Vec<char> = src.chars().collect();
    let (mut lits, mut code) = (Vec::new(), String::new());
    let mut i = 0;
    while i < c.len() {
        let next = c.get(i + 1).copied();
        if c[i] == '/' && next == Some('/') {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
        } else if c[i] == '/' && next == Some('*') {
            i += 2;
            while i < c.len() && !(c[i] == '*' && c.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i += 2;
        } else if c[i] == '\'' {
            // a char literal ('x', '\'', '\u{..}') or a lifetime ('a)
            if next == Some('\\') {
                // past the backslash and the escaped char, to the close
                i += 3;
                while i < c.len() && c[i] != '\'' {
                    i += 1;
                }
                i += 1;
            } else if c.get(i + 2) == Some(&'\'') {
                i += 3;
            } else {
                code.push(c[i]);
                i += 1;
            }
        } else if c[i] == 'r'
            && (i == 0 || !(c[i - 1].is_alphanumeric() || c[i - 1] == '_'))
            && matches!(next, Some('"' | '#'))
        {
            let mut j = i + 1;
            while c.get(j) == Some(&'#') {
                j += 1;
            }
            if c.get(j) != Some(&'"') {
                code.push(c[i]);
                i += 1;
                continue;
            }
            let hashes = j - i - 1;
            let close: Vec<char> = std::iter::once('"')
                .chain(std::iter::repeat_n('#', hashes))
                .collect();
            let start = j + 1;
            let mut end = start;
            while end < c.len() && !c[end..].starts_with(&close) {
                end += 1;
            }
            lits.push(c[start..end.min(c.len())].iter().collect());
            i = end + close.len();
        } else if c[i] == '"' {
            let mut lit = String::new();
            i += 1;
            while i < c.len() && c[i] != '"' {
                if c[i] == '\\' {
                    i += 1;
                    if i >= c.len() {
                        break;
                    }
                }
                lit.push(c[i]);
                i += 1;
            }
            i += 1;
            lits.push(lit);
        } else {
            code.push(c[i]);
            i += 1;
        }
    }
    (lits, code)
}

/// Does this literal read or write the table, as opposed to naming it? The
/// catalog lookups (`relname = 'attr_instances'`), the diagnostics and the
/// partition DDL name it without addressing its rows.
fn touches_rows(lit: &str) -> bool {
    [
        "FROM attr_instances",
        "INTO attr_instances",
        "UPDATE attr_instances",
        "FROM attr_rows",
        "INTO attr_rows",
        "UPDATE attr_rows",
    ]
    .iter()
    .any(|k| lit.contains(k))
}

/// Where the tenant has to appear. A statement that narrows rows says so in
/// a predicate, so `tenant_id` has to be at or after the first `WHERE` — a
/// mention in a SELECT list or a GROUP BY is not isolation. A statement with
/// no `WHERE` narrows nothing and only writes: there the tenant is a column
/// it fills, which is where the INSERT names it.
///
/// ponytail: this reads WHERE the name appears, not what it is compared to,
/// so a statement that mentions `tenant_id` in a predicate about some OTHER
/// table would pass. The failure it is built to catch is the one that
/// actually happens — a statement written with no tenant in it at all.
fn names_its_tenant(lit: &str) -> bool {
    match lit.find("WHERE") {
        Some(at) => lit[at..].contains("tenant_id"),
        None => lit.contains("tenant_id"),
    }
}

/// What one source file gets wrong: the statements on the table with no
/// tenant and no exemption, and whether the table is named somewhere the
/// scan cannot follow it into a statement — in code (a format argument, a
/// macro) or as a bare-name literal a statement is assembled from at
/// runtime. `used` marks the exemptions the file accounts for.
fn findings(src: &str, used: &mut [bool]) -> (Vec<String>, bool) {
    let (lits, code) = lex(src);
    let mut unread = code.contains("attr_instances") || code.contains("attr_rows");
    let mut naked = Vec::new();
    for lit in lits {
        if (lit.trim().starts_with("attr_instances") || lit.trim().starts_with("attr_rows"))
            && !lit.trim().contains(' ')
        {
            unread = true;
        }
        if !touches_rows(&lit) {
            continue;
        }
        if let Some(i) = EXEMPT.iter().position(|(frag, _)| lit.contains(frag)) {
            used[i] = true;
            continue;
        }
        if !names_its_tenant(&lit) {
            naked.push(lit);
        }
    }
    (naked, unread)
}

#[test]
fn every_statement_on_attr_instances_carries_its_tenant() {
    let mut files = Vec::new();
    sources(&store_dir(), &mut files);
    assert!(!files.is_empty(), "no store sources found");
    let mut naked = Vec::new();
    let mut unread = Vec::new();
    let mut used = vec![false; EXEMPT.len()];
    for file in &files {
        let src = std::fs::read_to_string(file).expect("read source");
        let (lits, blind) = findings(&src, &mut used);
        if blind {
            unread.push(file.display().to_string());
        }
        naked.extend(lits.into_iter().map(|l| format!("{}: {l}", file.display())));
    }
    assert!(
        unread.is_empty(),
        "attr_instances named where this check cannot see the statement it \
         ends up in (code, or a bare-name literal): {unread:?}"
    );
    assert!(
        naked.is_empty(),
        "attr_instances statements with no tenant predicate and no written \
         exemption — in timescale mode nothing else keeps tenants apart \
         (ADR-0006):\n{}",
        naked.join("\n\n")
    );
    for (i, (frag, _)) in EXEMPT.iter().enumerate() {
        assert!(
            used[i],
            "stale exemption: no statement contains {frag:?} any more"
        );
    }
}

/// The shapes that used to walk past the scan: a `'"'` char literal that
/// threw the old quote counting out of step for the rest of the file, a raw
/// string holding quotes, and the table name kept in a const or in code and
/// spliced into the statement at runtime.
#[test]
fn the_scan_is_not_blinded_by_lexical_shapes() {
    let judge = |src: &str| findings(src, &mut vec![false; EXEMPT.len()]);
    let after_quote_char =
        "fn f() { let q = '\"'; query(\"SELECT v FROM attr_instances WHERE id = $1\"); }";
    assert_eq!(judge(after_quote_char).0.len(), 1, "{after_quote_char}");
    let raw = "fn f() { query(r#\"SELECT \"v\" FROM attr_instances WHERE id = $1\"#); }";
    assert_eq!(judge(raw).0.len(), 1, "{raw}");
    let tenant = "fn f() { query(r#\"SELECT v FROM attr_instances WHERE tenant_id = $1\"#); }";
    assert_eq!(judge(tenant), (vec![], false), "{tenant}");
    let lifetime =
        "fn f<'a>(x: &'a str) { query(\"SELECT v FROM attr_instances WHERE id = $1\"); }";
    assert_eq!(judge(lifetime).0.len(), 1, "{lifetime}");
    let spliced = "const T: &str = \"attr_instances\"; fn f() { format!(\"SELECT v FROM {T}\"); }";
    assert!(judge(spliced).1, "{spliced}");
    let in_code = "fn f() { sql!(SELECT v FROM attr_instances); }";
    assert!(judge(in_code).1, "{in_code}");
    let commented = "// FROM attr_instances, said in a comment\nfn f() {}";
    assert_eq!(judge(commented), (vec![], false), "{commented}");
}
