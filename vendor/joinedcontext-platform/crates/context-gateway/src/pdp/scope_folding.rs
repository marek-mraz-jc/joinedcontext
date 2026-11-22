//! Hierarchical scopes as an anchored regex on the entity's own `scope` (T-0148, R13, R30).
//!
//! A grant on `/geo/SK/BB` covers the district and everything under it, and nothing
//! beside it. Sent as a separate `scopeQ` parameter that would be true, and unusable:
//! ADR 006 is about what happens when two policies apply and their parameters are merged
//! per parameter — the caller ends up with one policy's `q` and the other's scope, which
//! neither policy granted. R12 and R13 answer that by folding each policy's scopes into
//! that policy's own `q` term, so a policy is one conjunction and the policies are OR-ed
//! whole.
//!
//! The regex is anchored at both ends and the separator is explicit: `/geo/SK/BB` matches
//! `/geo/SK/BB` and `/geo/SK/BB/Sasova`, never `/geo/SK/BBB`, which is the whole point
//! (R30).

use antares_ql::scope::{scope_groups, subtree_scope_q};

/// The `q` term one `scopeQ` becomes, or `None` when it is no scope query.
///
/// Split and widened to subtrees by the broker's own parser (`antares_ql::scope`), so `;` is
/// AND and `|` is OR here as there: one anchored regex per pattern, joined the same way.
pub fn fold(scope_q: &str) -> Option<String> {
    let tree = subtree_scope_q(scope_q)?;
    let alternatives: Vec<String> = scope_groups(&tree)
        .map(|group| joined(group.map(term).collect(), ";"))
        .collect();
    Some(joined(alternatives, "|"))
}

/// `terms` as one term: parenthesized when they are several.
fn joined(terms: Vec<String>, operator: &str) -> String {
    match terms.as_slice() {
        [one] => one.clone(),
        several => format!("({})", several.join(operator)),
    }
}

/// One anchored regex term for one subtree pattern.
fn term(pattern: &str) -> String {
    if pattern == "/#" {
        return "scope~=\"^/.*$\"".to_owned();
    }
    let node = pattern.strip_suffix("/#").unwrap_or(pattern);
    // A level is a literal unless it is the one-level wildcard: a dot or a `+` inside a scope
    // name is a character, and a scope somebody names `a.b` must not match `axb`.
    let levels: Vec<String> = node
        .split('/')
        .map(|level| match level {
            "+" => "[^/]+".to_owned(),
            literal => regex::escape(literal),
        })
        .collect();
    format!("scope~=\"^{}(/.*)?$\"", levels.join("/"))
}
