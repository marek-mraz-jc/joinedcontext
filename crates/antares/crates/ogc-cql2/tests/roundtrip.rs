// SPDX-License-Identifier: EUPL-1.2
//! Property: an expression written as `cql2-text` parses back to the same tree.

use ogc_cql2::{parse, Expr, Geometry, Lang, Op};
use proptest::prelude::*;

fn scalar() -> impl Strategy<Value = Expr> {
    prop_oneof![
        "[a-z][a-zA-Z0-9_.]{0,6}"
            .prop_filter("reserved", |n| ![
                "and", "or", "not", "like", "between", "in", "is", "null", "true", "false", "div"
            ]
            .contains(&n.as_str()))
            .prop_map(Expr::Property),
        "[ a-zA-Z'\"%;|()]{0,6}".prop_map(Expr::Str),
        (-1.0e6..1.0e6f64).prop_map(Expr::Num),
        any::<bool>().prop_map(Expr::Bool),
        Just(Expr::Timestamp("2026-09-18T10:00:00.25Z".into())),
        Just(Expr::Date("2026-09-18".into())),
        Just(Expr::Interval(
            Box::new(Expr::Date("2026-09-18".into())),
            Box::new(Expr::Unbounded)
        )),
        Just(Expr::BBox(vec![-1.5, 2.0, 3.0, 4.0])),
        Just(Expr::Geometry(Geometry::MultiPolygon(vec![vec![vec![
            vec![0.0, 0.0],
            vec![1.0, 0.0, 2.0],
            vec![0.0, 0.0]
        ]]]))),
    ]
}

fn value() -> impl Strategy<Value = Expr> {
    scalar().prop_recursive(3, 16, 2, |inner| {
        let arith = prop_oneof![
            Just(Op::Add),
            Just(Op::Sub),
            Just(Op::Mul),
            Just(Op::Div),
            Just(Op::Mod),
            Just(Op::IntDiv),
            Just(Op::Pow)
        ];
        prop_oneof![
            (arith, inner.clone(), inner.clone()).prop_map(|(op, a, b)| Expr::Op(op, vec![a, b])),
            inner.clone().prop_map(|a| Expr::Op(Op::Casei, vec![a])),
            prop::collection::vec(inner, 0..3)
                .prop_map(|args| Expr::Op(Op::Function("fn1".into()), args)),
        ]
    })
}

fn predicate() -> impl Strategy<Value = Expr> {
    let cmp = prop_oneof![
        Just(Op::Eq),
        Just(Op::Ne),
        Just(Op::Lt),
        Just(Op::Le),
        Just(Op::Gt),
        Just(Op::Ge),
        Just(Op::Like),
        Just(Op::SWithin),
        Just(Op::TMetBy)
    ];
    let leaf = prop_oneof![
        (cmp, value(), value()).prop_map(|(op, a, b)| Expr::Op(op, vec![a, b])),
        (value(), value(), value()).prop_map(|(a, b, c)| Expr::Op(Op::Between, vec![a, b, c])),
        (value(), prop::collection::vec(value(), 0..3))
            .prop_map(|(a, l)| Expr::Op(Op::In, vec![a, Expr::Array(l)])),
        ("[a-z]{1,4}", prop::collection::vec(value(), 0..3))
            .prop_map(|(a, l)| Expr::Op(Op::AContains, vec![Expr::Property(a), Expr::Array(l)])),
        value().prop_map(|a| Expr::Op(Op::IsNull, vec![a])),
        any::<bool>().prop_map(Expr::Bool),
    ];
    leaf.prop_recursive(3, 24, 3, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 2..4).prop_map(|a| Expr::Op(Op::And, a)),
            prop::collection::vec(inner.clone(), 2..4).prop_map(|a| Expr::Op(Op::Or, a)),
            inner.prop_map(|a| Expr::Op(Op::Not, vec![a])),
        ]
    })
}

proptest! {
    #[test]
    fn text_rendering_parses_to_the_same_tree(tree in predicate()) {
        let written = tree.to_string();
        prop_assert_eq!(parse(Lang::Text, &written), Ok(tree), "{}", written);
    }
}
