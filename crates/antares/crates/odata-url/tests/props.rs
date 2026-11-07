// SPDX-License-Identifier: EUPL-1.2
//! AST → URL → AST (T-3503, T-3749): every tree of the subset writes itself as a URL that parses
//! back to the same tree, and that URL is its own canonical form.

mod service;

use odata_url::{
    parse, ArithOp, CompareOp, Compute, ExpandItem, ExpandKind, ExpandPath, Expr, Format,
    Geography, Key, KeyValue, Levels, Limits, Literal, MemberPath, Method, ODataUri, OrderBy,
    PartitionBy, Position, QueryOptions, Search, Segment, SelectItem, Shape, Transformation,
};
use proptest::prelude::*;
use service::Service;

#[cfg(test)]
mod strategy {
    use super::*;

    fn number_text() -> impl Strategy<Value = String> {
        prop_oneof![
            any::<i32>().prop_map(|i| i.to_string()),
            (any::<i16>(), 0_u16..1000).prop_map(|(i, f)| format!("{i}.{f}")),
            (any::<i16>(), -20_i8..20).prop_map(|(i, e)| format!("{i}e{e}")),
        ]
    }

    fn position() -> BoxedStrategy<Position> {
        prop::collection::vec(number_text(), 2..=4)
            .prop_map(Position)
            .boxed()
    }

    fn geography() -> BoxedStrategy<Geography> {
        let shape = prop_oneof![
            position().prop_map(Shape::Point),
            prop::collection::vec(prop::collection::vec(position(), 1..4), 1..3)
                .prop_map(Shape::Polygon),
        ];
        (0_u32..100_000, shape)
            .prop_map(|(srid, shape)| Geography { srid, shape })
            .boxed()
    }

    fn date() -> impl Strategy<Value = String> {
        (1000_u16..=9999, 1_u8..=12, 1_u8..=28)
            .prop_map(|(y, m, d)| format!("{y:04}-{m:02}-{d:02}"))
    }

    fn time() -> impl Strategy<Value = String> {
        (
            0_u8..24,
            0_u8..60,
            prop::option::of((0_u8..60, prop::option::of(1_u32..999_999))),
        )
            .prop_map(|(h, m, s)| match s {
                None => format!("{h:02}:{m:02}"),
                Some((s, None)) => format!("{h:02}:{m:02}:{s:02}"),
                Some((s, Some(f))) => format!("{h:02}:{m:02}:{s:02}.{f}"),
            })
    }

    pub(crate) fn literal() -> BoxedStrategy<Literal> {
        let offset = prop_oneof![
            Just("Z".to_owned()),
            ("[+-]", 0_u8..24, 0_u8..60).prop_map(|(s, h, m)| format!("{s}{h:02}:{m:02}")),
        ];
        prop_oneof![
            Just(Literal::Null),
            any::<bool>().prop_map(Literal::Bool),
            any::<i64>().prop_map(Literal::Int),
            number_text()
                .prop_filter("an integer is Int", |t| t.contains(['.', 'e']))
                .prop_map(Literal::Decimal),
            prop_oneof![Just("NaN"), Just("INF"), Just("-INF")]
                .prop_map(|w| Literal::Decimal(w.to_owned())),
            ".{0,12}".prop_map(Literal::String),
            date().prop_map(Literal::Date),
            (date(), time(), offset)
                .prop_map(|(d, t, o)| Literal::DateTimeOffset(format!("{d}T{t}{o}"))),
            time().prop_map(Literal::TimeOfDay),
            geography().prop_map(Literal::Geography),
        ]
        .boxed()
    }

    fn path(parts: &[&str]) -> Expr {
        Expr::Member(MemberPath {
            variable: None,
            path: parts.iter().map(|p| (*p).to_owned()).collect(),
        })
    }

    fn member() -> impl Strategy<Value = Expr> {
        prop_oneof![
            Just(path(&["height"])),
            Just(path(&["name"])),
            Just(path(&["address", "city"])),
            Just(path(&["owner", "name"])),
            Just(path(&["tags"])),
            Just(path(&["name", "@Core.Description"])),
            Just(path(&["@Core.Size#max"])),
        ]
    }

    fn compare() -> impl Strategy<Value = CompareOp> {
        prop_oneof![
            Just(CompareOp::Eq),
            Just(CompareOp::Ne),
            Just(CompareOp::Gt),
            Just(CompareOp::Ge),
            Just(CompareOp::Lt),
            Just(CompareOp::Le),
        ]
    }

    fn method() -> impl Strategy<Value = Method> {
        prop::sample::select(Method::all().collect::<Vec<_>>())
    }

    fn arith() -> impl Strategy<Value = ArithOp> {
        prop::sample::select(vec![
            ArithOp::Add,
            ArithOp::Sub,
            ArithOp::Mul,
            ArithOp::Div,
            ArithOp::DivBy,
            ArithOp::Mod,
        ])
    }

    fn lambda() -> impl Strategy<Value = Expr> {
        let area = Expr::Member(MemberPath {
            variable: Some("r".into()),
            path: vec!["area".into()],
        });
        let body = (compare(), any::<i64>()).prop_map(move |(op, n)| {
            Expr::Compare(
                Box::new(area.clone()),
                op,
                Box::new(Expr::Literal(Literal::Int(n))),
            )
        });
        let rooms = || MemberPath {
            variable: None,
            path: vec!["rooms".into()],
        };
        (prop::option::of(body), any::<bool>()).prop_map(move |(body, all)| match body {
            Some(b) if all => Expr::All {
                collection: rooms(),
                lambda: ("r".to_owned(), Box::new(b)),
            },
            body => Expr::Any {
                collection: rooms(),
                lambda: body.map(|b| ("r".to_owned(), Box::new(b))),
            },
        })
    }

    pub(crate) fn expr() -> BoxedStrategy<Expr> {
        let leaf = prop_oneof![
            literal().prop_map(Expr::Literal),
            member(),
            Just(Expr::Alias("p".into())),
            lambda(),
        ];
        leaf.prop_recursive(4, 24, 3, |inner| {
            prop_oneof![
                (inner.clone(), compare(), inner.clone()).prop_map(|(l, op, r)| Expr::Compare(
                    Box::new(l),
                    op,
                    Box::new(r)
                )),
                (inner.clone(), inner.clone())
                    .prop_map(|(l, r)| Expr::And(Box::new(l), Box::new(r))),
                (inner.clone(), inner.clone())
                    .prop_map(|(l, r)| Expr::Or(Box::new(l), Box::new(r))),
                inner.clone().prop_map(|e| Expr::Not(Box::new(e))),
                (inner.clone(), prop::collection::vec(literal(), 0..4))
                    .prop_map(|(l, list)| Expr::In(Box::new(l), list)),
                (method(), inner.clone(), inner.clone()).prop_map(|(m, a, b)| {
                    let args = [a, b].into_iter().take(m.arity().0).collect();
                    Expr::Call(m, args)
                }),
                (inner.clone(), arith(), inner.clone()).prop_map(|(l, op, r)| Expr::Arith(
                    Box::new(l),
                    op,
                    Box::new(r)
                )),
                inner.prop_map(|e| Expr::Neg(Box::new(e))),
            ]
        })
        .boxed()
    }

    fn select() -> impl Strategy<Value = Vec<SelectItem>> {
        let item = prop_oneof![
            Just(SelectItem::Star),
            Just(SelectItem::Path(vec!["name".into()])),
            Just(SelectItem::Path(vec!["address".into(), "city".into()])),
            Just(SelectItem::Path(vec!["sensors".into()])),
            Just(SelectItem::Annotation("@Core.Messages".into())),
        ];
        prop::collection::vec(item, 1..4)
    }

    fn expand() -> impl Strategy<Value = Vec<ExpandItem>> {
        let levels = prop::option::of(prop_oneof![
            Just(Levels::Max),
            (1_u64..=3).prop_map(Levels::Count)
        ]);
        let sensors =
            (any::<bool>(), any::<bool>(), levels.clone()).prop_map(|(select, owner, levels)| {
                ExpandItem {
                    path: ExpandPath::Path(vec!["sensors".into()]),
                    kind: ExpandKind::Entities {
                        select: select.then(|| vec![SelectItem::Path(vec!["value".into()])]),
                        expand: owner.then(|| {
                            vec![ExpandItem {
                                path: ExpandPath::Path(vec!["owner".into()]),
                                kind: ExpandKind::Ref,
                            }]
                        }),
                        levels,
                    },
                }
            });
        let item = prop_oneof![
            sensors,
            Just(ExpandItem {
                path: ExpandPath::Path(vec!["owner".into()]),
                kind: ExpandKind::Ref,
            }),
            Just(ExpandItem {
                path: ExpandPath::Star,
                kind: ExpandKind::Ref,
            }),
            levels.prop_map(|levels| ExpandItem {
                path: ExpandPath::Star,
                kind: ExpandKind::Entities {
                    select: None,
                    expand: None,
                    levels,
                },
            }),
        ];
        prop::collection::vec(item, 1..4)
    }

    fn format() -> impl Strategy<Value = Format> {
        prop_oneof![
            Just(Format::Json),
            Just(Format::Xml),
            Just(Format::Atom),
            Just(Format::MediaType(
                "application/json;odata.metadata=none".into()
            )),
        ]
    }

    /// A `$search` tree: words with every character a word may hold, an operator's name
    /// only as a phrase, phrases with spaces and quotes.
    pub(crate) fn search() -> BoxedStrategy<Search> {
        let word = "[a-zA-Z0-9é+&#%=!*,:@/?$~._-][a-zA-Z0-9é'+&#%=!*,:@/?$~._-]{0,6}"
            .prop_filter("an operator's name", |w| {
                !["AND", "OR", "NOT"].contains(&w.as_str())
            })
            .prop_map(Search::Word);
        let phrase = "[a-z' ()&;+é]{0,5}[a-z]".prop_map(Search::Phrase);
        prop_oneof![word, phrase]
            .boxed()
            .prop_recursive(4, 16, 2, |inner| {
                prop_oneof![
                    inner.clone().prop_map(|s| Search::Not(Box::new(s))),
                    (inner.clone(), inner.clone())
                        .prop_map(|(a, b)| Search::And(Box::new(a), Box::new(b))),
                    (inner.clone(), inner).prop_map(|(a, b)| Search::Or(Box::new(a), Box::new(b))),
                ]
            })
            .boxed()
    }

    /// The name of a custom option: never a system option's, with or without `$`.
    fn custom() -> impl Strategy<Value = (String, Option<String>)> {
        ("x-[a-z]{1,6}", prop::option::of("[a-z0-9.~-]{0,8}"))
    }

    pub(crate) fn options() -> BoxedStrategy<QueryOptions> {
        let orderby = prop::collection::vec((member(), any::<bool>()), 1..4)
            .prop_map(|items| {
                items
                    .into_iter()
                    .map(|(expr, descending)| OrderBy { expr, descending })
                    .collect()
            })
            .boxed();
        // the transformations that keep the entities, so the other options' names resolve
        let step = prop_oneof![
            Just(Transformation::Identity),
            expr().prop_map(Transformation::Filter),
            orderby.clone().prop_map(Transformation::OrderBy),
            any::<u64>().prop_map(Transformation::Top),
            any::<u64>().prop_map(Transformation::Skip),
            (expr(), expr(), any::<bool>(), 0..3u8).prop_map(|(n, expr, top, by)| {
                let by = match by {
                    0 => PartitionBy::Count,
                    1 => PartitionBy::Sum,
                    _ => PartitionBy::Percent,
                };
                Transformation::Partition { by, top, n, expr }
            }),
        ];
        let system = (
            prop::option::of(expr()),
            prop::option::of(select()),
            prop::option::of(expand()),
            prop::option::of(orderby),
            prop::option::of(any::<u64>()),
            prop::option::of(any::<u64>()),
            prop::option::of(any::<bool>()),
            prop::option::of(format()),
        );
        let rest = (
            prop::option::of("[a-zA-Z0-9]{1,8}"),
            prop::option::of(prop_oneof![Just("*".to_owned()), "[a-z0-9.]{1,6}"]),
            prop::option::of("[a-z]{1,8}"),
            prop::option::of(search()),
            prop::collection::vec(("[a-z]{1,4}", literal()), 0..3),
            prop::collection::vec(custom(), 0..3),
            prop::collection::vec(expr(), 0..3),
            prop::option::of(prop::collection::vec(step, 1..4)),
        );
        (system, rest)
            .prop_map(
                |(
                    (filter, select, expand, orderby, top, skip, count, format),
                    (skiptoken, schemaversion, id, search, aliases, custom, computed, apply),
                )| {
                    let compute = (!computed.is_empty()).then(|| {
                        computed
                            .into_iter()
                            .enumerate()
                            .map(|(i, expr)| Compute {
                                expr,
                                alias: format!("c{i}"),
                            })
                            .collect()
                    });
                    QueryOptions {
                        filter,
                        search,
                        compute,
                        apply,
                        select,
                        expand,
                        orderby,
                        top,
                        skip,
                        count,
                        format,
                        skiptoken,
                        schemaversion,
                        id,
                        aliases,
                        custom,
                    }
                },
            )
            .boxed()
    }

    fn key() -> impl Strategy<Value = Key> {
        let value = prop_oneof![
            literal().prop_map(KeyValue::Literal),
            "[a-z]{1,4}".prop_map(KeyValue::Alias),
        ];
        prop_oneof![
            value.clone().prop_map(Key::Single),
            (value.clone(), value)
                .prop_map(|(a, b)| Key::Compound(vec![("id".into(), a), ("height".into(), b)])),
        ]
    }

    pub(crate) fn uri() -> BoxedStrategy<ODataUri> {
        let format_only = || {
            prop::option::of(format()).prop_map(|format| QueryOptions {
                format,
                ..QueryOptions::default()
            })
        };
        let building = || Segment::EntitySet("Buildings".into());
        let tails = prop_oneof![
            Just(vec![]),
            Just(vec![Segment::Count]),
            Just(vec![Segment::Ref]),
            Just(vec![Segment::Query]),
            key().prop_map(|k| vec![Segment::Key(k)]),
            key().prop_map(|k| vec![
                Segment::Key(k),
                Segment::Property("name".into()),
                Segment::Value
            ]),
            (key(), key()).prop_map(|(a, b)| vec![
                Segment::Key(a),
                Segment::Navigation("sensors".into()),
                Segment::Key(b),
                Segment::Navigation("owner".into()),
                Segment::Ref,
            ]),
            key().prop_map(|k| vec![
                Segment::Key(k),
                Segment::Property("address".into()),
                Segment::Property("city".into())
            ]),
        ];
        // Options resolve against the building; a deeper path gets none.
        let resource = prop_oneof![
            (options(), prop::option::of(key())).prop_map(move |(options, key)| {
                ODataUri::Resource {
                    path: std::iter::once(building())
                        .chain(key.map(Segment::Key))
                        .collect(),
                    options,
                }
            }),
            tails.prop_map(move |tail| ODataUri::Resource {
                path: std::iter::once(building()).chain(tail).collect(),
                options: QueryOptions::default(),
            }),
        ];
        prop_oneof![
            Just(ODataUri::ServiceDocument),
            format_only().prop_map(|options| ODataUri::Metadata { options }),
            format_only().prop_map(|options| ODataUri::Batch { options }),
            (format_only(), "[a-z]{1,8}").prop_map(|(options, id)| ODataUri::Entity {
                options: QueryOptions {
                    id: Some(id),
                    ..options
                },
            }),
            resource,
        ]
        .boxed()
    }
}

#[cfg(test)]
proptest! {
    #![proptest_config(ProptestConfig::with_cases(if cfg!(miri) { 4 } else { 2000 }))]

    #[test]
    fn a_tree_writes_itself_and_parses_back(uri in strategy::uri()) {
        let text = uri.to_string();
        let again = parse(&text, &Service, &Limits::strict());
        prop_assert_eq!(again.as_ref(), Ok(&uri), "{}", text);
        prop_assert_eq!(again.map(|a| a.to_string()), Ok(text));
    }
}
