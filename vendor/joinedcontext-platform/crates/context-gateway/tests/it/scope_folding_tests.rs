//! Hierarchical scopes folded into the query as an anchored regex (T-0148, R12, R13, R30).

use context_gateway::auth::subject::Subject;
use context_gateway::pdp::evaluator::{evaluate, Request};
use context_gateway::pdp::scope_folding::fold;
use jc_core::kinds::{Operation, PolicySpec};
use jc_time::TimestampTz;
use regex::Regex;

fn now() -> TimestampTz {
    "2026-09-06T12:00:00Z".parse().expect("a fixed instant")
}

fn policy(yaml: &str) -> PolicySpec {
    serde_norway::from_str(yaml).expect("the policy spec parses")
}

/// The regex inside a folded term, compiled the way a broker would compile it.
fn pattern(term: &str) -> Regex {
    let (_, rest) = term.split_once("scope~=\"").expect("a scope term");
    let (inner, _) = rest.split_once('"').expect("a closed pattern");
    Regex::new(inner).expect("the folded pattern compiles")
}

/// R30: a grant on the district covers the district and what is under it, and nothing
/// that merely starts with the same letters.
#[test]
fn children_match_and_siblings_and_prefixes_do_not() {
    let term = fold("/geo/SK/BB").expect("one path folds");
    assert_eq!(term, r#"scope~="^/geo/SK/BB(/.*)?$""#);

    let scope = pattern(&term);
    assert!(scope.is_match("/geo/SK/BB"));
    assert!(scope.is_match("/geo/SK/BB/Center"));
    assert!(scope.is_match("/geo/SK/BB/Center/Namestie"));

    assert!(!scope.is_match("/geo/SK/BA"), "a sibling district");
    assert!(
        !scope.is_match("/geo/SK/BBB"),
        "a longer name is not a child"
    );
    assert!(
        !scope.is_match("/geo/SK"),
        "the parent is not covered by the child"
    );
    assert!(!scope.is_match("x/geo/SK/BB"), "anchored at the start");
    assert!(
        !scope.is_match("/geo/SK/BB-old"),
        "the separator is a slash, nothing else"
    );
}

/// The scope query language separates paths with its own operators; a grant on two
/// districts is an OR of two anchored terms, and the wildcard suffix is the same subtree.
#[test]
fn several_paths_fold_to_an_or_and_wildcards_are_trimmed() {
    let term = fold("/geo/SK/BB/#|/geo/SK/ZA/").expect("two paths fold");
    assert_eq!(
        term,
        r#"(scope~="^/geo/SK/BB(/.*)?$"|scope~="^/geo/SK/ZA(/.*)?$")"#
    );
    assert_eq!(
        fold("(/geo/SK/BB;/geo/SK/BB/Center)")
            .expect("folds")
            .matches("scope~=")
            .count(),
        2
    );

    assert_eq!(fold(""), None);
    assert_eq!(
        fold("geo/SK/BB"),
        None,
        "a path starts with a slash or it is not one"
    );
    assert_eq!(fold("/"), None, "the root alone is not a grant on anything");
}

/// A scope name is a literal: the characters a regex would read as operators are escaped,
/// so `a.b` matches `a.b` and never `axb`.
#[test]
fn regex_operators_in_a_scope_name_are_literal() {
    let term = fold("/org/dept.a+b").expect("folds");
    let scope = pattern(&term);
    assert!(scope.is_match("/org/dept.a+b/team"));
    assert!(!scope.is_match("/org/deptxa+b"));
    assert!(!scope.is_match("/org/dept.ab"));
}

/// R12 and ADR 006: two policies with different scopes and different filters. Each
/// policy's filter and scope travel in one term, so the merged query can never pair one
/// policy's `q` with the other's scope.
#[test]
fn a_policys_filter_and_scope_stay_in_one_term() {
    let bb = policy(
        r#"contextSpaceRef: ovzdusie
assigner: did:web:banskabystrica.sk
assignee: { kind: role, id: public }
operations: [queryEntity]
information:
  - entities:
      - type: AirQualityObserved
q: "pm10>=0"
scopeQ: "/geo/SK/BB"
"#,
    );
    let za = policy(
        r#"contextSpaceRef: ovzdusie
assigner: did:web:banskabystrica.sk
assignee: { kind: role, id: public }
operations: [queryEntity]
information:
  - entities:
      - type: AirQualityObserved
q: "pm25>=0"
scopeQ: "/geo/SK/ZA"
"#,
    );

    let request = Request {
        q: Some("dateObserved>2026".to_owned()),
        ..Request::default()
    };
    let verdict = evaluate(
        &Subject::anonymous(),
        Operation::QueryEntity,
        &request,
        "ovzdusie",
        &[bb, za],
        now(),
    );
    let constraints = verdict.constraints().expect("a rewrite");

    assert_eq!(
        constraints.q.as_deref(),
        Some(concat!(
            "(dateObserved>2026);",
            r#"((((pm10>=0);scope~="^/geo/SK/BB(/.*)?$"))"#,
            "|",
            r#"(((pm25>=0);scope~="^/geo/SK/ZA(/.*)?$")))"#,
        ))
    );
    assert_eq!(
        constraints.granted_scopes.as_deref(),
        Some("/geo/SK/BB|/geo/SK/ZA"),
        "the write guard sees the union of the granted trees, and nothing the caller sent"
    );
}

/// R30: a caller's own `scopeQ` never reaches the granted scope tree. Sending
/// `scopeQ=/geo/SK/ZA` on a write against a grant on BB must not make ZA writable.
#[test]
fn the_callers_scope_never_widens_the_granted_tree() {
    let bb = policy(
        r#"contextSpaceRef: ovzdusie
assigner: did:web:banskabystrica.sk
assignee: { kind: role, id: public }
operations: [createEntity, queryEntity]
information:
  - entities:
      - type: AirQualityObserved
scopeQ: "/geo/SK/BB"
"#,
    );
    let request = Request {
        scope_q: Some("/geo/SK/ZA".to_owned()),
        ..Request::default()
    };
    let verdict = evaluate(
        &Subject::anonymous(),
        Operation::CreateEntity,
        &request,
        "ovzdusie",
        std::slice::from_ref(&bb),
        now(),
    );
    let constraints = verdict.constraints().expect("a rewrite");

    assert_eq!(constraints.granted_scopes.as_deref(), Some("/geo/SK/BB"));
    let entity = serde_json::json!({
        "id": "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:za-1",
        "type": "AirQualityObserved",
        "scope": "/geo/SK/ZA",
    });
    assert!(
        context_gateway::pdp::write_guard::check(&entity, constraints).is_err(),
        "a write into a district the caller only named itself is refused"
    );
}

/// S2-17 (T-7543): `;` is AND, in either spelling, for the folded read and the write guard.
#[test]
fn a_conjunction_admits_only_entities_in_both_scopes() {
    for granted in ["/a;/b", "(/a;/b)"] {
        let term = fold(granted).expect("folds");
        assert_eq!(
            term, r#"(scope~="^/a(/.*)?$";scope~="^/b(/.*)?$")"#,
            "{granted}"
        );
        let constraints = context_gateway::pdp::evaluator::Constraints {
            granted_scopes: Some(granted.to_owned()),
            ..Default::default()
        };
        for (scope, admitted) in [
            (serde_json::json!(["/a", "/b"]), true),
            (serde_json::json!(["/a/x", "/b"]), true),
            (serde_json::json!("/a"), false),
            (serde_json::json!(["/b"]), false),
            (serde_json::json!(["/a", "/b", "/c"]), false),
            (serde_json::json!([]), false),
        ] {
            let entity = serde_json::json!({
                "id": "urn:ngsi-ld:AirQualityObserved:city.example:air:a1", "type": "AirQualityObserved", "scope": scope,
            });
            assert_eq!(
                context_gateway::pdp::write_guard::check(&entity, &constraints).is_ok(),
                admitted,
                "{granted}: {entity}"
            );
        }
    }
}

/// S2-17: a `scopeQ` that is no scope query grants nothing, where it used to grant everything.
#[test]
fn a_scope_query_that_does_not_parse_grants_nothing() {
    for broken in ["geo/SK", "(/a;/b", "/a/#/b", ""] {
        let unreadable = policy(&format!(
            "contextSpaceRef: air\nassigner: did:web:city.example\nassignee: {{ kind: role, id: public }}\noperations: [queryEntity]\nscopeQ: \"{broken}\"\n"
        ));
        let verdict = evaluate(
            &Subject::anonymous(),
            Operation::QueryEntity,
            &Request::default(),
            "air",
            &[unreadable],
            now(),
        );
        let q = verdict.constraints().and_then(|c| c.q.clone());
        assert!(
            q.as_deref()
                .is_some_and(|q| q.contains(context_gateway::pdp::evaluator::NOTHING)),
            "{broken:?}: {q:?}"
        );
    }
}

/// The splitter against the broker's parser, exhaustively: for every query of up to two
/// alternatives of up to two patterns, on every set of the scopes below, the folded `q` as
/// the broker parses it selects what the broker's `scopeQ` selects over the subtrees, and a
/// write lies inside the grant only where a read would select it.
#[test]
fn a_folded_scope_selects_what_the_brokers_parser_selects() {
    use antares_ql::scope::{scope_matches, scopes_within, subtree_scope_q};
    use antares_ql::{parse_q, QNode, QValue};

    fn holds(node: &QNode, scopes: &[&str]) -> bool {
        match node {
            QNode::And(all) => all.iter().all(|one| holds(one, scopes)),
            QNode::Or(any) => any.iter().any(|one| holds(one, scopes)),
            QNode::Cmp {
                value: QValue::Str(regex),
                ..
            } => {
                let regex = antares_ql::regex::compile(regex).expect("a folded pattern compiles");
                scopes.iter().any(|scope| regex.is_match(scope))
            }
            other => panic!("not a folded scope term: {other:?}"),
        }
    }

    const PATTERNS: [&str; 7] = ["/a", "/b", "/a/b", "/a/+", "/+/b", "/#", "/a.b/"];
    const SCOPES: [&str; 7] = ["/a", "/b", "/a/b", "/a/b/c", "/ab", "/a.b", "/axb"];
    let mut groups: Vec<String> = PATTERNS.map(str::to_owned).to_vec();
    for left in PATTERNS {
        groups.extend(PATTERNS.map(|right| format!("({left};{right})")));
    }
    let mut queries = groups.clone();
    for left in &groups {
        for operator in [",", "|"] {
            queries.extend(
                groups
                    .iter()
                    .map(|right| format!("{left}{operator}{right}")),
            );
        }
    }
    let sets: Vec<Vec<&str>> = (0..1_u32 << SCOPES.len())
        .map(|bits| {
            SCOPES
                .iter()
                .enumerate()
                .filter(|(at, _)| bits & (1 << at) != 0)
                .map(|(_, scope)| *scope)
                .collect()
        })
        .collect();
    let stored: Vec<serde_json::Value> = sets
        .iter()
        .map(|scopes| serde_json::json!({ "scope": scopes }))
        .collect();
    for query in &queries {
        let tree = subtree_scope_q(query).expect("a scope query");
        let folded = parse_q(&fold(query).expect("a scope query folds")).expect("a q");
        for (scopes, document) in sets.iter().zip(&stored) {
            let read = holds(&folded, scopes);
            assert_eq!(
                read,
                scope_matches(&tree, document),
                "{query} on {scopes:?}"
            );
            assert!(
                read || !scopes_within(&tree, scopes),
                "{query} on {scopes:?}"
            );
        }
    }
}
