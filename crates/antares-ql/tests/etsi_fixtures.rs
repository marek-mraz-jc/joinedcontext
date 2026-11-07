// SPDX-License-Identifier: EUPL-1.2
//! The query languages over the ETSI NGSI-LD test suite's own fixtures and TP tables
//! (`ngsi-ld-test-suite`, vendored in the workspace; not in the published crate).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
use antares_jsonld::{core_context, expand_entity, Context, ExpandOpts};
use ngsild_ql::eval::eval_q;
use ngsild_ql::parse_q;
use ngsild_ql::scope::{check_scope_q, scope_matches};
use serde_json::Value;
use std::path::{Path, PathBuf};

fn suite() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ngsi-ld-test-suite")
}

fn read_json(rel: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(suite().join(rel)).unwrap()).unwrap()
}

/// The rows of a Robot `*** Test Cases ***    SCOPEQ    EXPECTED_COUNT` template table.
fn scope_rows(tp: &str) -> Vec<(String, usize)> {
    let robot = std::fs::read_to_string(suite().join(tp)).unwrap();
    robot
        .lines()
        .filter_map(|l| {
            let mut cols = l.strip_prefix("    ")?.split("    ");
            let (q, n) = (cols.next()?, cols.next()?);
            let n = n.strip_prefix("${")?.strip_suffix('}')?.parse().ok()?;
            q.starts_with(['/', '(']).then(|| (q.to_owned(), n))
        })
        .collect()
}

/// 019_01_06 and 019_34: every scopeQ the TPs send is inside the 4.19 grammar and selects as
/// many of the two suite Entities (one Scope, two Scopes) as the TP expects.
#[test]
fn the_tp_scope_queries_check_and_select_the_expected_entities() {
    let entities = [
        read_json("data/entities/building-minimal-with-one-scope.json"),
        read_json("data/entities/building-minimal-with-many-scopes.json"),
    ];
    let mut rows = scope_rows(
        "TP/NGSI-LD/ContextInformation/Consumption/Entity/QueryEntities/019_01_06.robot",
    );
    rows.extend(scope_rows(
        "TP/NGSI-LD/ContextInformation/Consumption/Entity/QueryEntities/019_34.robot",
    ));
    assert!(rows.len() >= 10, "{rows:?}");
    for (q, expected) in rows {
        check_scope_q(&q).unwrap_or_else(|e| panic!("{q}: {e}"));
        let selected = entities.iter().filter(|e| scope_matches(&q, e)).count();
        assert_eq!(selected, expected, "{q}");
    }
}

fn suite_context() -> Context {
    let mut ctx = core_context();
    ctx.merge_object(
        read_json("resources/jsonld-contexts/ngsi-ld-test-suite.jsonld")["@context"]
            .as_object()
            .unwrap(),
    )
    .unwrap();
    ctx.freeze();
    ctx
}

fn entity(rel: &str, ctx: &Context) -> Value {
    let Value::Object(mut doc) = read_json(rel) else {
        panic!("{rel}")
    };
    doc.remove("@context");
    expand_entity(&doc, ctx, ExpandOpts::default()).unwrap()
}

/// `q=` expressions the QueryEntities and Subscription TPs send, over the suite's vehicle
/// (speed 56, no heading) and building fixtures.
#[test]
fn the_tp_q_expressions_parse_and_evaluate_over_the_suite_entities() {
    let ctx = suite_context();
    let vehicle = entity("data/entities/vehicle-speed-attribute.jsonld", &ctx);
    for (q, holds) in [
        ("speed>25", true),
        ("speed>100", false),
        ("speed!=10", true),
        ("speed==10..40", false),
        ("speed==50..60", true),
        ("!heading;speed>25", true),
        ("speed>25|heading>100", true),
        ("speed", true),
        ("heading", false),
        ("speed.source==\"Speedometer\"", true),
    ] {
        let parsed = parse_q(q).unwrap_or_else(|e| panic!("{q}: {e}"));
        assert_eq!(eval_q(&parsed, &vehicle, &ctx, &|_| None), holds, "{q}");
    }
    // Every `q` a suite Subscription fixture carries parses.
    let dir = suite().join("data/subscriptions");
    let mut subscription_qs = 0;
    for f in std::fs::read_dir(&dir).unwrap() {
        let path = f.unwrap().path();
        let Ok(Value::Object(sub)) =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_default())
        else {
            continue;
        };
        if let Some(q) = sub.get("q").and_then(Value::as_str) {
            parse_q(q).unwrap_or_else(|e| panic!("{}: {q}: {e}", path.display()));
            subscription_qs += 1;
        }
    }
    assert!(subscription_qs >= 1);
    for refused in ["speed>", "speed==", "(speed>1"] {
        assert_eq!(parse_q(refused).unwrap_err().status(), 400, "{refused}");
    }
}
