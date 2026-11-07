// SPDX-License-Identifier: EUPL-1.2
//! Scope Query Language (CIM 009 clause 4.19), evaluated in memory over the
//! entity's `scope` member.

use antares_model::NgsiError;
use serde_json::Value;

/// Scope Query evaluation (4.19) — `|`/`,` = OR, `(a;b)` = AND (parenthesis
/// grouping), `+` one level, trailing `#` the subtree incl. the node, `/#`
/// any non-empty scope.
pub fn scope_matches(scope_q: &str, doc: &Value) -> bool {
    // scope is an array in the entity internal form, but a bare string is
    // legal on documents stored verbatim (e.g. registrations, 5.2.9)
    let scopes: Vec<&str> = match doc.get("scope") {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
        Some(Value::String(s)) => vec![s.as_str()],
        _ => Vec::new(),
    };
    scope_q.split([',', '|']).any(|and_group| {
        and_group
            .trim()
            .trim_start_matches('(')
            .trim_end_matches(')')
            .split(';')
            .all(|pat| scopes.iter().any(|s| scope_pattern_matches(pat.trim(), s)))
    })
}

/// Checks a `scopeQ` against the 4.19 grammar (TS 104 175 7.2.5) before it is
/// evaluated: a disjunction (`,` or `|`) of ScopeQs and parenthesized
/// `;`-conjunctions of them, each `/level(/level)*` with `+` a whole level and
/// `#` only as the last one, or `/#`; a level starts with a letter. A bare
/// conjunction (`/a;/b`) is a 400 like any other text outside the grammar. An empty query, an empty group or pattern (`/a,,/b`,
/// `/a;;/b`) or an empty level (`//a`) is a 400 BadRequestData: without this
/// [`scope_matches`] skips the empty piece and answers a wider query than the
/// one asked. Longer than [`MAX_SCOPE_Q_BYTES`] is a 403 TooComplexQuery.
///
/// # Errors
/// [`NgsiError::BadRequestData`] or [`NgsiError::TooComplexQuery`] as above.
pub fn check_scope_q(scope_q: &str) -> Result<(), NgsiError> {
    if scope_q.len() > MAX_SCOPE_Q_BYTES {
        return Err(NgsiError::TooComplexQuery(format!(
            "scopeQ is longer than {MAX_SCOPE_Q_BYTES} bytes"
        )));
    }
    let bad = |why: &str| NgsiError::BadRequestData(format!("invalid scopeQ {scope_q:?}: {why}"));
    for group in scope_q.split([',', '|']) {
        let group = group.trim();
        let inner = match (group.strip_prefix('('), group.strip_suffix(')')) {
            (Some(_), Some(_)) => group.get(1..group.len().saturating_sub(1)).unwrap_or(""),
            // TS 104 175 7.2.5: "For logical and grouping parenthesis are needed"
            (None, None) if group.contains(';') => {
                return Err(bad("a ';' conjunction needs parentheses, e.g. (/a;/b)"));
            }
            (None, None) => group,
            _ => return Err(bad("unbalanced parenthesis")),
        };
        if inner.is_empty() {
            return Err(bad("empty group"));
        }
        for pat in inner.split(';') {
            check_scope_pattern(pat.trim()).map_err(bad)?;
        }
    }
    Ok(())
}

fn check_scope_pattern(pat: &str) -> Result<(), &'static str> {
    if pat.is_empty() {
        return Err("empty pattern");
    }
    let Some(rest) = pat.strip_prefix('/') else {
        return Err("a ScopeQ starts with '/'");
    };
    let levels: Vec<&str> = rest.split('/').collect();
    let last = levels.len().saturating_sub(1);
    for (i, level) in levels.iter().enumerate() {
        match *level {
            "" => return Err("empty scope level"),
            "#" if i != last => return Err("'#' is only the last level"),
            "#" | "+" => {}
            // ScopeQLevel = unicodeLetter *(unicodeNumber / unicodeLetter / "_"), the
            // grammar every stored Scope already follows (antares-jsonld valid_scope_value)
            l => {
                let mut ch = l.chars();
                if !(ch.next().is_some_and(char::is_alphabetic)
                    && ch.all(|c| c.is_alphabetic() || c.is_numeric() || c == '_'))
                {
                    return Err("a scope level is a letter, then letters, digits or '_'");
                }
            }
        }
    }
    Ok(())
}

/// The Scope Query that selects what BOTH arguments select (4.19).
///
/// A Scope Query is a disjunction of conjunctions, and the conjunction is
/// over predicates that are independent of each other — each pattern asks
/// whether SOME Scope of the Entity matches it — so `and` distributes over
/// `or` and the intersection is a disjunction of the pairwise unions:
/// `(a1,a2)` and `(b1,b2)` select what `(a1;b1),(a1;b2),(a2;b1),(a2;b2)`
/// selects. Every term of the result is a `;`-conjunction of plain
/// `ScopeQ`s, which is what the grammar's parenthesized `OrScopeQ` derives,
/// so the answer is a Scope Query and not a broker-private structure.
///
/// `None` when either side contributes no group, or when the product would
/// exceed [`MAX_SCOPE_Q_BYTES`]: the caller then has an intersection it
/// cannot express and must refuse rather than serve the wider of the two.
pub fn intersect_scope_q(a: &str, b: &str) -> Option<String> {
    let groups = |s: &str| -> Vec<String> {
        s.split([',', '|'])
            .map(|g| {
                g.trim()
                    .trim_start_matches('(')
                    .trim_end_matches(')')
                    .trim()
            })
            .filter(|g| !g.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let (ga, gb) = (groups(a), groups(b));
    if ga.is_empty() || gb.is_empty() {
        return None;
    }
    let mut out = String::new();
    for x in &ga {
        for y in &gb {
            if !out.is_empty() {
                out.push(',');
            }
            // `/#` selects every Entity that carries any Scope at all, so it
            // adds nothing to a conjunction that already names one — and it
            // is the one ScopesQ alternative the grammar does not derive
            // inside a parenthesized group.
            match (x.as_str(), y.as_str()) {
                (ANY_SCOPE, ANY_SCOPE) => out.push_str(ANY_SCOPE),
                (ANY_SCOPE, term) | (term, ANY_SCOPE) => out.push_str(term),
                (l, r) => {
                    out.push('(');
                    out.push_str(l);
                    out.push(';');
                    out.push_str(r);
                    out.push(')');
                }
            }
            if out.len() > MAX_SCOPE_Q_BYTES {
                return None;
            }
        }
    }
    Some(out)
}

/// The ScopesQ that selects every Entity carrying a non-empty Scope (4.19).
const ANY_SCOPE: &str = "/#";

/// Ceiling on a Scope Query this crate will build. The store's own compiler
/// declines a longer one and leaves it to the in-memory arbiter; an
/// intersection past it is refused instead, so no caller is handed a query
/// that is quietly wider than the one it asked for.
pub const MAX_SCOPE_Q_BYTES: usize = 4096;

/// One 4.19 ScopeQ against one Entity Scope: `/`-separated levels compared
/// in order, `+` standing for any single level and a trailing `#` for the
/// rest of the hierarchy including the node itself.
fn scope_pattern_matches(pat: &str, scope: &str) -> bool {
    if pat == ANY_SCOPE {
        return true;
    }
    let pseg: Vec<&str> = pat.split('/').filter(|s| !s.is_empty()).collect();
    let sseg: Vec<&str> = scope.split('/').filter(|s| !s.is_empty()).collect();
    let mut i = 0;
    for (pi, p) in pseg.iter().enumerate() {
        if *p == "#" {
            // multi-level wildcard: matches the rest (including nothing)
            return pi.saturating_add(1) == pseg.len();
        }
        let Some(sv) = sseg.get(i) else { return false };
        if *p != "+" && p != sv {
            return false;
        }
        i = i.saturating_add(1);
    }
    i == sseg.len()
}

#[cfg(test)]
mod clause_4_19 {
    use super::{check_scope_q, intersect_scope_q, scope_matches, MAX_SCOPE_Q_BYTES};
    use serde_json::json;

    fn doc(scopes: &[&str]) -> serde_json::Value {
        json!({"id": "urn:x", "type": ["T"], "scope": scopes})
    }

    /// 4.19 EXAMPLES 1-3: direct scope, `#` subtree (including the node
    /// itself), `+` single-level wildcard, `/#` any non-empty scope.
    #[test]
    fn wildcards_and_direct_scopes() {
        assert!(scope_matches("/Madrid", &doc(&["/Madrid"])));
        assert!(!scope_matches("/Madrid", &doc(&["/Madrid/Gardens"])));
        for s in [
            "/Madrid/Gardens",
            "/Madrid/Gardens/ParqueNorte",
            "/Madrid/Gardens/ParqueNorte/Parterre1",
        ] {
            assert!(scope_matches("/Madrid/Gardens/#", &doc(&[s])), "{s}");
        }
        assert!(!scope_matches(
            "/Madrid/Gardens/#",
            &doc(&["/Madrid/Sights"])
        ));
        assert!(scope_matches(
            "/Madrid/+/ParqueNorte",
            &doc(&["/Madrid/Sights/ParqueNorte"])
        ));
        assert!(!scope_matches(
            "/Madrid/+/ParqueNorte",
            &doc(&["/Madrid/ParqueNorte"])
        ));
        assert!(scope_matches("/#", &doc(&["/Anything"])));
        assert!(
            !scope_matches("/#", &doc(&[])),
            "no scope = no match for /#"
        );
    }

    /// 4.19 EXAMPLES 4/5: conjunction needs parentheses; disjunction is `|`
    /// OR the compatibility comma.
    #[test]
    fn conjunction_and_both_or_spellings() {
        let both = doc(&["/Madrid/Districts", "/CompanyA"]);
        let only_b = doc(&["/CompanyB"]);
        let only_madrid = doc(&["/Madrid/Districts"]);
        assert!(scope_matches("(/Madrid/Districts;/CompanyA)", &both));
        assert!(
            !scope_matches("(/Madrid/Districts;/CompanyA)", &only_madrid),
            "conjunction requires ALL scopes"
        );
        for sel in [
            "(/Madrid/Districts;/CompanyA)|/CompanyB",
            "(/Madrid/Districts;/CompanyA),/CompanyB",
        ] {
            assert!(scope_matches(sel, &both), "{sel}");
            assert!(scope_matches(sel, &only_b), "{sel}");
            assert!(!scope_matches(sel, &only_madrid), "{sel}");
        }
    }

    /// The ABNF puts `andOp` inside the parenthesized `OrScopeQ` alone, so
    /// `(a;b),(c;d)` is the only spelling it derives, and [`check_scope_q`]
    /// refuses the bare `a;b` at every entry (forge cc8711ce moved
    /// `019_01_06_05` to the parenthesized form and added `019_03_07_05`).
    /// [`scope_matches`] still evaluates a bare conjunction an internal
    /// caller builds, and the two spellings have to MEAN the same thing:
    /// `;` binds tighter than `,`/`|`, the reading the parentheses force.
    #[test]
    fn a_conjunction_means_the_same_parenthesized_or_bare() {
        let docs = [
            doc(&[]),
            doc(&["/A"]),
            doc(&["/B"]),
            doc(&["/C"]),
            doc(&["/A", "/B"]),
            doc(&["/C", "/D"]),
            doc(&["/A", "/D"]),
            doc(&["/A", "/B", "/C", "/D"]),
        ];
        for d in &docs {
            assert_eq!(
                scope_matches("(/A;/B),(/C;/D)", d),
                scope_matches("/A;/B,/C;/D", d),
                "the two spellings disagree on {d}"
            );
            assert_eq!(
                scope_matches("(/A;/B)|(/C;/D)", d),
                scope_matches("/A;/B|/C;/D", d),
                "the two spellings disagree on {d}"
            );
        }
        // and the grouping is the one the parentheses state, not the other
        // one: `/A;/B,/C` is `(/A AND /B) OR /C`, never `/A AND (/B OR /C)`.
        assert!(scope_matches("/A;/B,/C", &doc(&["/C"])));
        assert!(!scope_matches("/A;/B,/C", &doc(&["/A"])));
        assert!(scope_matches("/A;/B,/C", &doc(&["/A", "/B"])));
    }

    /// The intersection has to select what BOTH select, for every Entity —
    /// a policy engine narrowing a request that brought its own Scope Query
    /// is the caller, and an intersection that is wider than either side is
    /// a disclosure.
    #[test]
    fn an_intersection_selects_exactly_what_both_select() {
        let docs = [
            doc(&[]),
            doc(&["/A"]),
            doc(&["/B"]),
            doc(&["/BB"]),
            doc(&["/BB/Traffic"]),
            doc(&["/A", "/B"]),
            doc(&["/A", "/BB/Traffic"]),
            doc(&["/BB", "/BB/Traffic"]),
            doc(&["/A", "/B", "/BB", "/BB/Traffic"]),
        ];
        for (a, b) in [
            ("/A", "/B"),
            ("/A,/B", "/B,/C"),
            ("/BB", "/BB/Traffic"),
            ("/BB/#", "/BB/Traffic"),
            ("(/A;/B)", "/BB/#"),
            ("(/A;/B),/BB", "(/BB;/BB/Traffic),/A"),
            ("/#", "/A,/B"),
            ("/#", "/#"),
            ("/A", "/A"),
        ] {
            let both = intersect_scope_q(a, b).expect("expressible");
            for d in &docs {
                assert_eq!(
                    scope_matches(&both, d),
                    scope_matches(a, d) && scope_matches(b, d),
                    "{a} INTERSECT {b} = {both} disagrees on {d}"
                );
            }
        }
    }

    /// Nothing to intersect, and an intersection too large to write down:
    /// both leave the caller to refuse rather than serve the wider side.
    #[test]
    fn an_inexpressible_intersection_is_none() {
        assert_eq!(intersect_scope_q("", "/A"), None);
        assert_eq!(intersect_scope_q("/A", "  "), None);
        let wide = (0..200)
            .map(|n| format!("/S{n}"))
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(intersect_scope_q(&wide, &wide), None, "200x200 groups");
    }

    /// A Scope stored as a bare string (a registration kept verbatim) is
    /// matched like a one-element array; the intersection may be exactly
    /// MAX_SCOPE_Q_BYTES long, one byte more is refused (T-3531).
    #[test]
    fn bare_string_scope_and_the_intersection_ceiling() {
        use serde_json::json;
        assert!(scope_matches("/a", &json!({"scope": "/a"})));
        assert!(!scope_matches("/b", &json!({"scope": "/a"})));
        let at = |n: usize| format!("/{}", "s".repeat(n - 1));
        assert_eq!(
            intersect_scope_q("/#", &at(super::MAX_SCOPE_Q_BYTES)).map(|s| s.len()),
            Some(super::MAX_SCOPE_Q_BYTES)
        );
        assert_eq!(
            intersect_scope_q("/#", &at(super::MAX_SCOPE_Q_BYTES + 1)),
            None
        );
    }

    /// 4.19 grammar: the shapes the examples use pass; an empty piece, a
    /// level after `#`, or a missing leading `/` is a 400, never a silent
    /// widening; past the ceiling is a 403.
    #[test]
    fn check_scope_q_follows_the_grammar() {
        for ok in [
            "/Madrid",
            "/Madrid/+/ParqueNorte",
            "/Madrid/#",
            "/#",
            "/Madrid,/Berlin",
            "/Madrid|/Berlin",
            "(/Madrid;/Gardens),/Berlin",
            "(/Madrid/Gardens/ParqueNorte;/CompanyA/OrganizationB/UnitC)",
            "/Madrid/Gardens/ParqueNorte/#",
            "/Madrid/+/#",
            "/Ma_drid2/Été",
        ] {
            assert!(check_scope_q(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "/a,,/b",
            "/a;;/b",
            "//a",
            "/a/",
            "Madrid",
            "/a/#/b",
            "(/a;/b",
            "()",
            "/a b",
            "/a#",
            // TS 104 175 7.2.5 (forge 019_03_07): "For logical and grouping parenthesis are
            // needed", and a ScopeQLevel is a unicodeLetter then letters, numbers or "_"
            "/a;/b",
            "/Madrid;",
            "/a;/b,/c",
            "/Madrid/1st",
            "/Madrid/+G",
            "/a.b",
            "/a-b",
            // an empty pattern inside a group
            "(/a;;/b)",
            "(/Madrid;)",
        ] {
            let err = check_scope_q(bad).unwrap_err();
            assert_eq!(err.status(), 400, "{bad:?}: {err}");
        }
        let long = "/a,".repeat(MAX_SCOPE_Q_BYTES);
        assert_eq!(check_scope_q(&long).unwrap_err().status(), 403);
    }
}
