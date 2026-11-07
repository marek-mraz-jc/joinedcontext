// SPDX-License-Identifier: EUPL-1.2
//! Threat O7 (URL parser exhaustion): every limit of `Limits` is refused before the parser reads,
//! and so before it copies or allocates for, anything past the bound. The proof is black-box:
//! whatever follows the point where a limit is exceeded never changes the answer, so those bytes
//! were never looked at.

mod service;

use odata_url::{parse, LimitKind, Limits, UrlError, UrlErrorKind};
use service::Service;

#[cfg(test)]
mod o7 {
    use super::*;

    const TINY: Limits = Limits {
        max_path_len: 4096,
        max_query_len: 4096,
        max_path_segments: 3,
        max_query_options: 3,
        max_expr_depth: 3,
        max_expr_nodes: 12,
        max_expand_depth: 2,
        max_expand_items: 3,
        max_select_items: 3,
        max_orderby_items: 3,
        max_in_list_len: 3,
        max_literal_len: 16,
    };

    /// Tails that would each change a parse that read them: nothing, a valid continuation,
    /// malformed bytes, and a long run of the same.
    fn tails() -> [String; 4] {
        [
            String::new(),
            ")".to_owned(),
            "'%(@$\u{e9}#".to_owned(),
            "x".repeat(2000),
        ]
    }

    fn refusal(uri: &str) -> UrlError {
        let refused = parse(uri, &Service, &TINY);
        assert!(refused.is_err(), "{uri}: {refused:?}");
        refused.err().unwrap_or(UrlError {
            kind: UrlErrorKind::Unresolved,
            offset: 0,
        })
    }

    #[test]
    fn every_limit_is_refused_before_it_allocates() {
        let deep_expand = "Buildings?$expand=sensors($expand=owner($expand=x";
        let cases = [
            (
                format!("Buildings('{}", "a".repeat(4100)),
                LimitKind::PathLength,
            ),
            (
                format!("Buildings?$filter='{}", "a".repeat(4100)),
                LimitKind::QueryLength,
            ),
            (
                "Buildings(1)/sensors(1)/owner/".to_owned(),
                LimitKind::PathSegments,
            ),
            ("Buildings?a=1&b=1&c=1&".to_owned(), LimitKind::QueryOptions),
            ("Buildings?$filter=((((".to_owned(), LimitKind::ExprDepth),
            (
                "Buildings?$filter=height eq 1 or height eq 1 or height eq 1 or height eq "
                    .to_owned(),
                LimitKind::ExprNodes,
            ),
            (deep_expand.to_owned(), LimitKind::ExpandDepth),
            (
                "Buildings?$expand=owner,owner,owner,".to_owned(),
                LimitKind::ExpandItems,
            ),
            (
                "Buildings?$select=name,name,name,".to_owned(),
                LimitKind::SelectItems,
            ),
            (
                "Buildings?$orderby=name,name,name,".to_owned(),
                LimitKind::OrderbyItems,
            ),
            (
                "Buildings?$filter=name in (1,2,3,".to_owned(),
                LimitKind::InListLength,
            ),
            (
                format!("Buildings?$filter=name eq '{}", "a".repeat(40)),
                LimitKind::LiteralLength,
            ),
            (
                format!("Buildings?$filter=height eq {}", "9".repeat(40)),
                LimitKind::LiteralLength,
            ),
            (
                format!("Buildings?$top={}", "9".repeat(30)),
                LimitKind::Number,
            ),
        ];
        for (prefix, limit) in &cases {
            let answers: Vec<UrlError> = tails()
                .iter()
                .map(|tail| refusal(&format!("{prefix}{tail}")))
                .collect();
            let first = answers.first().copied();
            assert!(
                matches!(first, Some(UrlError { kind: UrlErrorKind::LimitExceeded { limit: l, .. }, .. }) if l == *limit),
                "{prefix}: {first:?}"
            );
            assert!(
                answers.iter().all(|a| Some(*a) == first),
                "{prefix}: {answers:?}"
            );
        }
    }

    #[test]
    fn a_literal_at_its_bound_is_taken() {
        let at_bound = format!("Buildings?$filter=name eq '{}'", "a".repeat(14));
        let taken = parse(&at_bound, &Service, &TINY);
        assert!(taken.is_ok(), "{taken:?}");
        // An overlong date-time is refused, not read as the date it starts with; a short
        // number before malformed bytes is a number.
        let short = parse(
            "Buildings?$filter=height eq 2024-01-01T10:00:00.1234567+01:00",
            &Service,
            &TINY,
        );
        assert!(
            matches!(
                short,
                Err(UrlError {
                    kind: UrlErrorKind::LimitExceeded {
                        limit: LimitKind::LiteralLength,
                        ..
                    },
                    ..
                })
            ),
            "{short:?}"
        );
        let number = parse("Buildings?$filter=height eq 2024-01x", &Service, &TINY);
        assert!(
            matches!(
                number,
                Err(UrlError {
                    kind: UrlErrorKind::Syntax { .. },
                    ..
                })
            ),
            "{number:?}"
        );
        // The bound inside a percent escape: still too long, not malformed (fuzz, T-3503).
        for cut in ["'%2B%2B%2B%2B%2B%2B%2B'", "'a%2B%2B%2B%2B%2B%2B%2B'"] {
            let long = parse(&format!("Buildings?$filter=name eq {cut}"), &Service, &TINY);
            assert!(
                matches!(
                    long,
                    Err(UrlError {
                        kind: UrlErrorKind::LimitExceeded {
                            limit: LimitKind::LiteralLength,
                            ..
                        },
                        ..
                    })
                ),
                "{cut}: {long:?}"
            );
        }
    }
}

/// F-ANT5-001: a member path through a self-referencing single-valued navigation is walked in a
/// loop, each segment one level of `max_expr_depth`, so 4000 segments are a refusal, not a stack
/// overflow on a small thread.
#[cfg(test)]
mod member_path_depth {
    use odata_url::{parse, LimitKind, Limits, Member, Model, UrlError, UrlErrorKind};

    struct SelfRef;

    impl Model for SelfRef {
        fn entity_set(&self, name: &str) -> Option<&str> {
            (name == "Nodes").then_some("N")
        }

        fn member(&self, ty: &str, name: &str) -> Option<Member<'_>> {
            match (ty, name) {
                ("N", "p") => Some(Member::Navigation {
                    ty: "N",
                    collection: false,
                }),
                ("N", "x") => Some(Member::Primitive { collection: false }),
                _ => None,
            }
        }
    }

    fn parse_deep(segments: usize) -> Result<(), UrlError> {
        let uri = format!("Nodes?$filter={}x eq 1", "p/".repeat(segments));
        parse(&uri, &SelfRef, &Limits::strict()).map(|_| ())
    }

    #[test]
    fn a_long_self_referencing_path_is_refused_on_a_small_stack() {
        let worker = std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(|| parse_deep(4000));
        let answer = worker.map(std::thread::JoinHandle::join);
        let refused = matches!(
            answer,
            Ok(Ok(Err(UrlError {
                kind: UrlErrorKind::LimitExceeded {
                    limit: LimitKind::ExprDepth,
                    ..
                },
                ..
            })))
        );
        assert!(refused, "{answer:?}");
    }

    #[test]
    fn a_short_self_referencing_path_still_parses() {
        let short = parse_deep(8);
        assert!(short.is_ok(), "{short:?}");
    }
}
