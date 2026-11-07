// SPDX-License-Identifier: EUPL-1.2
//! Unit tests: every rule of the subset, its errors, its limits and the canonical writer.

use super::*;

/// Buildings with an address, rooms, tags, sensors and an owner; sensors with a value.
struct Service;

impl Model for Service {
    fn entity_set(&self, name: &str) -> Option<&str> {
        match name {
            "Buildings" => Some("Building"),
            "Sensors" => Some("Sensor"),
            _ => None,
        }
    }

    fn member(&self, ty: &str, name: &str) -> Option<Member<'_>> {
        let primitive = Member::Primitive { collection: false };
        Some(match (ty, name) {
            ("Building", "id" | "height" | "name" | "built" | "location")
            | ("Address", "street" | "city")
            | ("Room", "area")
            | ("Sensor", "value" | "at")
            | ("Person", "name") => primitive,
            ("Building", "tags") => Member::Primitive { collection: true },
            ("Building", "address") => Member::Complex {
                ty: "Address",
                collection: false,
            },
            ("Building", "rooms") => Member::Complex {
                ty: "Room",
                collection: true,
            },
            ("Building" | "Address", "sensors") => Member::Navigation {
                ty: "Sensor",
                collection: true,
            },
            ("Building" | "Sensor", "owner") => Member::Navigation {
                ty: "Person",
                collection: false,
            },
            ("Sensor", "building") => Member::Navigation {
                ty: "Building",
                collection: false,
            },
            _ => return None,
        })
    }
}

fn ok(uri: &str) -> ODataUri {
    let parsed = parse(uri, &Service, &Limits::strict());
    assert!(parsed.is_ok(), "{uri}: {parsed:?}");
    parsed.unwrap_or(ODataUri::ServiceDocument)
}

fn err(uri: &str) -> UrlError {
    err_within(uri, &Limits::strict())
}

fn err_within(uri: &str, limits: &Limits) -> UrlError {
    let parsed = parse(uri, &Service, limits);
    assert!(parsed.is_err(), "{uri}: {parsed:?}");
    parsed.err().unwrap_or(UrlError {
        kind: UrlErrorKind::Unresolved,
        offset: usize::MAX,
    })
}

fn options(uri: &str) -> QueryOptions {
    match ok(uri) {
        ODataUri::Resource { options, .. } => options,
        other => {
            assert!(
                matches!(other, ODataUri::Resource { .. }),
                "{uri}: {other:?}"
            );
            QueryOptions::default()
        }
    }
}

fn path(uri: &str) -> Vec<Segment> {
    match ok(uri) {
        ODataUri::Resource { path, .. } => path,
        other => {
            assert!(
                matches!(other, ODataUri::Resource { .. }),
                "{uri}: {other:?}"
            );
            Vec::new()
        }
    }
}

fn filter(text: &str) -> Expr {
    options(&format!("Buildings?$filter={text}"))
        .filter
        .unwrap_or(Expr::Literal(Literal::Null))
}

fn syntax(uri: &str, offset: usize) {
    let e = err(uri);
    assert!(
        matches!(e.kind, UrlErrorKind::Syntax { .. }),
        "{uri}: {e:?}"
    );
    assert_eq!(e.offset, offset, "{uri}: {e}");
}

fn refused(uri: &str, feature: Feature, offset: usize) {
    let e = err(uri);
    assert_eq!(e.kind, UrlErrorKind::NotImplemented(feature), "{uri}: {e}");
    assert_eq!(e.offset, offset, "{uri}: {e}");
}

fn unresolved(uri: &str, offset: usize) {
    let e = err(uri);
    assert_eq!(e.kind, UrlErrorKind::Unresolved, "{uri}: {e}");
    assert_eq!(e.offset, offset, "{uri}: {e}");
}

fn member(path: &[&str]) -> Expr {
    Expr::Member(MemberPath {
        variable: None,
        path: path.iter().map(|p| (*p).to_owned()).collect(),
    })
}

fn int(value: i64) -> Expr {
    Expr::Literal(Literal::Int(value))
}

fn string(text: &str) -> Literal {
    Literal::String(text.to_owned())
}

#[test]
fn the_service_forms() {
    assert_eq!(ok(""), ODataUri::ServiceDocument);
    assert_eq!(
        ok("$metadata"),
        ODataUri::Metadata {
            options: QueryOptions::default()
        }
    );
    assert_eq!(
        ok("$metadata?"),
        ODataUri::Metadata {
            options: QueryOptions::default()
        }
    );
    let json = QueryOptions {
        format: Some(Format::Json),
        ..QueryOptions::default()
    };
    assert_eq!(
        ok("$metadata?$format=json"),
        ODataUri::Metadata {
            options: json.clone()
        }
    );
    assert_eq!(ok("$batch?format=JSON"), ODataUri::Batch { options: json });
    assert_eq!(
        ok("$batch"),
        ODataUri::Batch {
            options: QueryOptions::default()
        }
    );
    let entity = ok("$entity?$id=Buildings('b1')&$format=xml");
    let ODataUri::Entity { options } = entity else {
        unreachable_form(&entity);
        return;
    };
    assert_eq!(options.id.as_deref(), Some("Buildings('b1')"));
    assert_eq!(options.format, Some(Format::Xml));
    // `$entity` takes `$id` and `$format` only; `$select` is no custom option either.
    syntax("$entity?$id=x&$select=name", 14);
    syntax("$entity?$format=json", 20);
    syntax("$entity", 7);
    refused("$entity/Model.Building?$id=x", Feature::TypeCast, 0);
    syntax("$metadata?$top=1", 10);
    refused("$crossjoin(Buildings,Sensors)", Feature::CrossJoin, 0);
    refused("$all", Feature::CrossJoin, 0);
    syntax("$metadata#Buildings", 9);
}

/// URL Conventions section 4 and the ABNF `resourcePath`: no path segment is
/// empty, so a trailing or doubled `/` is a syntax error at the empty segment,
/// never the resource without it and never a name that does not resolve
/// (T-3731).
#[test]
fn an_empty_path_segment_is_a_syntax_error() {
    syntax("Buildings/", 10);
    syntax("Buildings//", 10);
    syntax("Buildings//name", 10);
    syntax("Buildings/?$top=1", 10);
    syntax("Buildings('b1')/", 16);
    syntax("Buildings('b1')//name", 16);
}

fn unreachable_form(uri: &ODataUri) {
    assert!(matches!(uri, ODataUri::Entity { .. }), "{uri:?}");
}

#[test]
fn resource_paths() {
    let set = || Segment::EntitySet("Buildings".into());
    let single = |l: Literal| Segment::Key(Key::Single(KeyValue::Literal(l)));
    assert_eq!(path("Buildings"), vec![set()]);
    assert_eq!(path("Buildings('b1')"), vec![set(), single(string("b1"))]);
    assert_eq!(
        path("Buildings%28%27b1%27%29"),
        vec![set(), single(string("b1"))]
    );
    assert_eq!(path("Buildings(7)"), vec![set(), single(Literal::Int(7))]);
    assert_eq!(
        path("Buildings(@k)"),
        vec![
            set(),
            Segment::Key(Key::Single(KeyValue::Alias("k".into())))
        ]
    );
    assert_eq!(
        path("Buildings(id='b1',height=@h)"),
        vec![
            set(),
            Segment::Key(Key::Compound(vec![
                ("id".into(), KeyValue::Literal(string("b1"))),
                ("height".into(), KeyValue::Alias("h".into())),
            ]))
        ]
    );
    assert_eq!(path("Buildings/$count"), vec![set(), Segment::Count]);
    assert_eq!(path("Buildings/$ref"), vec![set(), Segment::Ref]);
    assert_eq!(path("Buildings/$query"), vec![set(), Segment::Query]);
    assert_eq!(
        path("Buildings('b1')/name/$value"),
        vec![
            set(),
            single(string("b1")),
            Segment::Property("name".into()),
            Segment::Value
        ]
    );
    assert_eq!(
        path("Buildings('b1')/address/city"),
        vec![
            set(),
            single(string("b1")),
            Segment::Property("address".into()),
            Segment::Property("city".into())
        ]
    );
    assert_eq!(
        path("Buildings('b1')/sensors(3)/owner/$ref"),
        vec![
            set(),
            single(string("b1")),
            Segment::Navigation("sensors".into()),
            single(Literal::Int(3)),
            Segment::Navigation("owner".into()),
            Segment::Ref,
        ]
    );
    assert_eq!(path("Buildings('b1')/rooms/$count").len(), 4);
    assert_eq!(path("Buildings('b1')/tags/$count").len(), 4);
    assert_eq!(path("Buildings('b1')/tags").len(), 3);
    assert_eq!(path("Buildings('b1')/rooms").len(), 3);
    assert_eq!(path("Buildings('b1')/sensors").len(), 3);
}

#[test]
fn resource_path_errors() {
    unresolved("Towers", 0);
    // As the OASIS case `Model.Rejection`: an action import is no resource of the subset.
    syntax("Model.Buildings", 5);
    syntax("('x')", 0);
    unresolved("Buildings/b1", 10);
    unresolved("Buildings/1", 10);
    unresolved("Buildings('b1')/width", 16);
    refused("Buildings/Model.Tower", Feature::TypeCast, 10);
    refused("Buildings('b1')/$value", Feature::MediaValue, 16);
    refused("Buildings/$filter(height gt 1)", Feature::FilterSegment, 10);
    refused("Buildings/$each", Feature::Each, 10);
    refused("Buildings('b1')/rooms/0", Feature::Ordinal, 22);
    refused("Buildings('b1')/tags/-1", Feature::Ordinal, 21);
    // After `$count` nothing follows; `$value` only after a single primitive.
    syntax("Buildings/$count/x", 16);
    syntax("Buildings('b1')/address/$value", 24);
    syntax("Buildings('b1')/name/x", 22);
    syntax("Buildings('b1')/tags/x", 22);
    syntax("Buildings('b1')/rooms/area", 26);
    syntax("Buildings(", 10);
    syntax("Buildings(id", 12);
    syntax("Buildings(id=", 13);
    syntax("Buildings(id='b1'", 17);
    syntax("Buildings(id='b1';height=1)", 17);
    syntax("Buildings('b1'", 14);
    syntax("Buildings(@)", 11);
    syntax("Buildings('b1')x", 15);
}

#[test]
fn query_options() {
    let o = options("Buildings?$top=5&skip=2&$count=true&$skiptoken=abc%20d&$schemaversion=1.0&@h=10&mode=fast&debug");
    assert_eq!((o.top, o.skip, o.count), (Some(5), Some(2), Some(true)));
    assert_eq!(o.skiptoken.as_deref(), Some("abc%20d"));
    assert_eq!(o.schemaversion.as_deref(), Some("1.0"));
    assert_eq!(o.aliases, vec![("h".to_owned(), Literal::Int(10))]);
    assert_eq!(
        o.custom,
        vec![
            ("mode".to_owned(), Some("fast".to_owned())),
            ("debug".to_owned(), None)
        ]
    );
    assert_eq!(options("Buildings?$count=FALSE").count, Some(false));
    assert_eq!(
        options("Buildings?$schemaversion=*")
            .schemaversion
            .as_deref(),
        Some("*")
    );
    assert_eq!(options("Buildings?$format=atom").format, Some(Format::Atom));
    assert_eq!(
        options("Buildings?$format=application/json;odata.metadata=none").format,
        Some(Format::MediaType(
            "application/json;odata.metadata=none".into()
        ))
    );
    assert_eq!(
        options("Buildings?%6Dode=x").custom,
        vec![("mode".to_owned(), Some("x".to_owned()))]
    );
    assert_eq!(options("Buildings?%C3%A9=x").custom.len(), 1);
    assert_eq!(options("Buildings?").top, None);
    let dup = err("Buildings?$top=1&top=2");
    assert_eq!(
        (dup.kind, dup.offset),
        (UrlErrorKind::DuplicateOption("$top"), 17)
    );
}

#[test]
fn query_option_errors() {
    syntax("Buildings?$top=x", 15);
    syntax("Buildings?$count=yes", 17);
    syntax("Buildings?$format=", 18);
    syntax("Buildings?$format=a/", 20);
    syntax("Buildings?$skiptoken=", 21);
    syntax("Buildings?$schemaversion=", 25);
    syntax("Buildings?$id=", 14);
    syntax("Buildings?=x", 10);
    syntax("Buildings?$x=1", 10);
    syntax("Buildings?$top=1$skip=1", 16);
    syntax("Buildings?$top =1", 14);
    syntax("Buildings?@=1", 11);
    syntax("Buildings?@h", 12);
    refused("Buildings?@h=height", Feature::AliasExpression, 13);
    refused("Buildings?@h=1 add 2", Feature::AliasExpression, 13);
    let big = err("Buildings?$top=99999999999999999999");
    assert_eq!(
        big.kind,
        UrlErrorKind::LimitExceeded {
            limit: LimitKind::Number,
            max: usize::MAX
        }
    );
    for (name, feature) in [
        ("$index=1", Feature::Index),
        ("$deltatoken=x", Feature::DeltaToken),
    ] {
        refused(&format!("Buildings?{name}"), feature, 10);
    }
    // `$deltatoken` needs its `$`: without it it is a custom option.
    assert_eq!(options("Buildings?deltatoken=x").custom.len(), 1);
}

#[test]
fn filter_expressions() {
    assert_eq!(
        filter("height gt 10"),
        Expr::Compare(
            Box::new(member(&["height"])),
            CompareOp::Gt,
            Box::new(int(10))
        )
    );
    for (word, op) in [
        ("eq", CompareOp::Eq),
        ("ne", CompareOp::Ne),
        ("ge", CompareOp::Ge),
        ("lt", CompareOp::Lt),
        ("le", CompareOp::Le),
    ] {
        assert_eq!(
            filter(&format!("height {word} 1")),
            Expr::Compare(Box::new(member(&["height"])), op, Box::new(int(1)))
        );
    }
    let a = || {
        Box::new(Expr::Compare(
            Box::new(member(&["height"])),
            CompareOp::Gt,
            Box::new(int(1)),
        ))
    };
    let b = || {
        Box::new(Expr::Compare(
            Box::new(member(&["height"])),
            CompareOp::Lt,
            Box::new(int(9)),
        ))
    };
    assert_eq!(filter("height gt 1 and height lt 9"), Expr::And(a(), b()));
    assert_eq!(filter("height gt 1 or height lt 9"), Expr::Or(a(), b()));
    assert_eq!(filter("not (height gt 1)"), Expr::Not(a()));
    assert_eq!(filter("not%20(height%20gt%201)"), Expr::Not(a()));
    assert_eq!(
        filter("height gt 1 or height lt 9 and height lt 9"),
        Expr::Or(a(), Box::new(Expr::And(b(), b())))
    );
    assert_eq!(
        filter("(height gt 1 or height lt 9) and height lt 9"),
        Expr::And(Box::new(Expr::Or(a(), b())), b())
    );
    assert_eq!(
        filter("name in ('a', 'b')"),
        Expr::In(Box::new(member(&["name"])), vec![string("a"), string("b")])
    );
    assert_eq!(
        filter("name in ()"),
        Expr::In(Box::new(member(&["name"])), vec![])
    );
    assert_eq!(filter("@p"), Expr::Alias("p".into()));
    assert_eq!(
        filter("address/city eq 'Bonn'"),
        Expr::Compare(
            Box::new(member(&["address", "city"])),
            CompareOp::Eq,
            Box::new(Expr::Literal(string("Bonn")))
        )
    );
    assert_eq!(filter("owner/name"), member(&["owner", "name"]));
    assert_eq!(filter("tags"), member(&["tags"]));
    assert_eq!(filter("rooms"), member(&["rooms"]));
    assert_eq!(
        filter("name/@Core.Description"),
        member(&["name", "@Core.Description"])
    );
    assert_eq!(filter("@Core.Messages"), member(&["@Core.Messages"]));
    assert_eq!(
        filter("tags/@Core.Size%23max"),
        member(&["tags", "@Core.Size#max"])
    );
    assert_eq!(filter("true"), Expr::Literal(Literal::Bool(true)));
}

#[test]
fn methods_and_lambdas() {
    let call = |m: Method, path: &[&str], text: &str| {
        Expr::Call(m, vec![member(path), Expr::Literal(string(text))])
    };
    assert_eq!(
        filter("contains(name,'x')"),
        call(Method::Contains, &["name"], "x")
    );
    assert_eq!(
        filter("startswith( name , 'x' )"),
        call(Method::StartsWith, &["name"], "x")
    );
    assert_eq!(
        filter("ENDSWITH(name,'x')"),
        call(Method::EndsWith, &["name"], "x")
    );
    let geo = filter("geo.intersects(location,geography'SRID=4326;Point(1 2)')");
    assert!(
        matches!(geo, Expr::Call(Method::GeoIntersects, _)),
        "{geo:?}"
    );
    let any = filter("rooms/any(r:r/area gt 10)");
    let Expr::Any {
        collection,
        lambda: Some((variable, body)),
    } = any
    else {
        assert!(matches!(any, Expr::Any { .. }), "{any:?}");
        return;
    };
    assert_eq!(
        (collection.path, variable.as_str()),
        (vec!["rooms".to_owned()], "r")
    );
    assert_eq!(
        *body,
        Expr::Compare(
            Box::new(Expr::Member(MemberPath {
                variable: Some("r".into()),
                path: vec!["area".into()]
            })),
            CompareOp::Gt,
            Box::new(int(10))
        )
    );
    assert_eq!(
        filter("tags/any()"),
        Expr::Any {
            collection: MemberPath {
                variable: None,
                path: vec!["tags".into()]
            },
            lambda: None
        }
    );
    assert_eq!(filter("tags/any( )"), filter("tags/any()"));
    let nested = filter("sensors/any(s:s/owner/name eq 'x' and tags/any(t:t eq 'y'))");
    assert!(matches!(nested, Expr::Any { .. }), "{nested:?}");
    let bare = filter("tags/any(t:t)");
    let Expr::Any {
        lambda: Some((_, body)),
        ..
    } = bare
    else {
        assert!(matches!(bare, Expr::Any { .. }), "{bare:?}");
        return;
    };
    assert_eq!(
        *body,
        Expr::Member(MemberPath {
            variable: Some("t".into()),
            path: vec![]
        })
    );
}

#[test]
fn filter_errors() {
    let at = |text: &str| "Buildings?$filter=".len().saturating_add(text.len());
    syntax("Buildings?$filter=", 18);
    syntax("Buildings?$filter=height gt", 27);
    syntax("Buildings?$filter=(height gt 1", at("(height gt 1"));
    syntax("Buildings?$filter=contains(name)", at("contains(name"));
    syntax(
        "Buildings?$filter=contains(name,'x'",
        at("contains(name,'x'"),
    );
    syntax("Buildings?$filter=name in ('a'", at("name in ('a'"));
    syntax("Buildings?$filter=name/x", at("name/x"));
    syntax("Buildings?$filter=tags/x", at("tags/x"));
    syntax("Buildings?$filter=tags/any(t)", at("tags/any(t"));
    syntax("Buildings?$filter=tags/any(t:t", at("tags/any(t:t"));
    syntax("Buildings?$filter=tags/any(:t)", at("tags/any("));
    syntax("Buildings?$filter=width(1)", at("width"));
    syntax("Buildings?$filter=Model.f.", at("Model.f."));
    syntax("Buildings?$filter=@Core.", at("@Core."));
    syntax(
        "Buildings?$filter=name/@Core.Size%23",
        at("name/@Core.Size%23"),
    );
    unresolved("Buildings?$filter=width gt 1", 18);
    unresolved("Buildings?$filter=@Size/x", 18);
    unresolved("Buildings?$filter=@Core/x", 18);
    unresolved("Buildings?$filter=address/width", 26);
    let base = "Buildings?$filter=".len();
    for (text, feature, offset) in [
        ("$root/Buildings", Feature::Root, 0),
        ("$it/height", Feature::ImplicitVariable, 0),
        ("$this", Feature::ImplicitVariable, 0),
        ("height has 1", Feature::Has, 7),
        ("name in tags", Feature::InExpression, 8),
        ("name in (name)", Feature::InExpression, 8),
        ("totalseconds(name) gt 1", Feature::Method, 0),
        ("hassubset(tags,tags)", Feature::Method, 0),
        ("geo.distance(location,location) lt 1", Feature::Method, 0),
        ("cast(name,Edm.String)", Feature::CastIsOf, 0),
        ("isof(Model.Tower)", Feature::CastIsOf, 0),
        ("[1,2]", Feature::JsonLiteral, 0),
        ("{\"a\":1}", Feature::JsonLiteral, 0),
        ("Model.f(1)", Feature::Operation, 0),
        ("Model.Tower", Feature::TypeCast, 0),
        ("name/Model.f()", Feature::Operation, 5),
        ("address/Model.Ext/city", Feature::TypeCast, 8),
        ("tags/Model.Ext", Feature::TypeCast, 5),
        ("sensors/count()", Feature::Operation, 8),
        ("tags/$count gt 1", Feature::NestedCount, 4),
        ("tags/$filter(true)", Feature::PathInExpression, 4),
        ("sensors(1)/value", Feature::PathInExpression, 7),
        ("@Core.Messages/any()", Feature::AnnotationPath, 0),
        ("name/@Core.Size/x", Feature::AnnotationPath, 5),
        ("tags/@Core.Size/x", Feature::AnnotationPath, 5),
        (
            "height gt geometry'SRID=0;Point(1 1)'",
            Feature::Literal,
            10,
        ),
    ] {
        refused(
            &format!("Buildings?$filter={text}"),
            feature,
            base.saturating_add(offset),
        );
    }
}

#[test]
fn select_and_orderby() {
    let o = options("Buildings?$select=*,name,address/city,@Core.Messages,address,tags,owner&$orderby=height desc,name asc,id");
    assert_eq!(
        o.select,
        Some(vec![
            SelectItem::Star,
            SelectItem::Path(vec!["name".into()]),
            SelectItem::Path(vec!["address".into(), "city".into()]),
            SelectItem::Annotation("@Core.Messages".into()),
            SelectItem::Path(vec!["address".into()]),
            SelectItem::Path(vec!["tags".into()]),
            SelectItem::Path(vec!["owner".into()]),
        ])
    );
    let by = o.orderby.unwrap_or_default();
    assert_eq!(
        by.iter().map(|o| o.descending).collect::<Vec<_>>(),
        vec![true, false, false]
    );
    unresolved("Buildings?$select=width", 18);
    unresolved("Buildings?$select=address/width", 26);
    syntax("Buildings?$select=", 18);
    syntax("Buildings?$select=name,", 23);
    refused("Buildings?$select=Model.Tower/name", Feature::TypeCast, 18);
    refused(
        "Buildings?$select=address($select=city)",
        Feature::NestedOption,
        26,
    );
    refused("Buildings?$select=tags($top=1)", Feature::NestedOption, 23);
    refused(
        "Buildings?$select=@Core.Messages($top=1)",
        Feature::NestedOption,
        33,
    );
    refused(
        "Buildings?$select=address/@Core.Messages",
        Feature::AnnotationPath,
        26,
    );
}

#[test]
fn expand() {
    let o = options("Buildings?$expand=sensors($select=value;$expand=owner;$levels=2),owner/$ref,*,*/$ref,*($levels=max),address/sensors");
    let items = o.expand.unwrap_or_default();
    assert_eq!(items.len(), 6);
    assert_eq!(
        items.first(),
        Some(&ExpandItem {
            path: ExpandPath::Path(vec!["sensors".into()]),
            kind: ExpandKind::Entities {
                select: Some(vec![SelectItem::Path(vec!["value".into()])]),
                expand: Some(vec![ExpandItem {
                    path: ExpandPath::Path(vec!["owner".into()]),
                    kind: ExpandKind::Entities {
                        select: None,
                        expand: None,
                        levels: None
                    },
                }]),
                levels: Some(Levels::Count(2)),
            },
        })
    );
    assert_eq!(items.get(1).map(|i| &i.kind), Some(&ExpandKind::Ref));
    assert_eq!(items.get(2).map(|i| &i.path), Some(&ExpandPath::Star));
    assert_eq!(items.get(3).map(|i| &i.kind), Some(&ExpandKind::Ref));
    assert_eq!(
        items.get(4).map(|i| &i.kind),
        Some(&ExpandKind::Entities {
            select: None,
            expand: None,
            levels: Some(Levels::Max)
        })
    );
    assert_eq!(
        items.get(5).map(|i| &i.path),
        Some(&ExpandPath::Path(vec!["address".into(), "sensors".into()]))
    );
}

#[test]
fn expand_errors() {
    let base = "Buildings?$expand=".len();
    for (text, feature, offset) in [
        ("$value", Feature::MediaValue, 0),
        ("@Core.Messages", Feature::AnnotationPath, 0),
        ("Model.Tower/sensors", Feature::TypeCast, 0),
        ("sensors/Model.Probe", Feature::TypeCast, 8),
        ("sensors/Probe", Feature::TypeCast, 8),
        ("sensors/$count", Feature::NestedCount, 7),
        ("sensors($filter=true)", Feature::NestedOption, 8),
        ("sensors($top=1;$select=value)", Feature::NestedOption, 8),
        ("sensors(@a=1)", Feature::NestedOption, 8),
        ("sensors/$ref($top=1)", Feature::NestedOption, 13),
        ("address/*", Feature::PathStar, 8),
    ] {
        refused(
            &format!("Buildings?$expand={text}"),
            feature,
            base.saturating_add(offset),
        );
    }
    unresolved("Buildings?$expand=width", 18);
    unresolved("Buildings?$expand=name", 18);
    syntax("Buildings?$expand=", 18);
    syntax("Buildings?$expand=address", 25);
    syntax("Buildings?$expand=sensors($levels=0)", 34);
    syntax("Buildings?$expand=sensors($select=value", 39);
    syntax("Buildings?$expand=sensors($x=1)", 26);
    syntax("Buildings?$expand=sensors/$ref($levels=1)", 31);
    syntax("Buildings?$expand=*(", 20);
    syntax("Buildings?$expand=*($levels=1", 29);
    let deep = err("Buildings?$expand=sensors($levels=4)");
    assert_eq!(
        deep.kind,
        UrlErrorKind::LimitExceeded {
            limit: LimitKind::ExpandDepth,
            max: 3
        }
    );
    assert_eq!(deep.offset, 34);
    let nested = err("Buildings?$expand=sensors($expand=building($expand=sensors($expand=owner)))");
    assert_eq!(
        nested.kind,
        UrlErrorKind::LimitExceeded {
            limit: LimitKind::ExpandDepth,
            max: 3
        }
    );
}

#[test]
fn literals_in_keys_and_filters() {
    for (text, literal) in [
        ("null", Literal::Null),
        ("false", Literal::Bool(false)),
        ("-12", Literal::Int(-12)),
        ("1.5", Literal::Decimal("1.5".into())),
        ("2024-02-29", Literal::Date("2024-02-29".into())),
        (
            "2024-02-29T10:00:00Z",
            Literal::DateTimeOffset("2024-02-29T10:00:00Z".into()),
        ),
        (
            "2024-02-29T10%3A00%2B01%3A00",
            Literal::DateTimeOffset("2024-02-29T10:00+01:00".into()),
        ),
        ("10:00:00.5", Literal::TimeOfDay("10:00:00.5".into())),
        ("'it''s'", string("it's")),
        ("'%C3%A9'", string("\u{e9}")),
    ] {
        assert_eq!(filter(text), Expr::Literal(literal.clone()), "{text}");
        assert_eq!(
            path(&format!("Buildings({text})")).get(1),
            Some(&Segment::Key(Key::Single(KeyValue::Literal(literal)))),
            "{text}"
        );
    }
}

#[test]
fn limits_are_checked() {
    let tiny = Limits {
        max_path_len: 20,
        max_query_len: 30,
        max_path_segments: 2,
        max_query_options: 2,
        max_expr_depth: 2,
        max_expr_nodes: 6,
        max_expand_depth: 1,
        max_expand_items: 2,
        max_select_items: 2,
        max_orderby_items: 2,
        max_in_list_len: 2,
        max_literal_len: 6,
    };
    let limit = |uri: &str, kind: LimitKind| {
        let e = err_within(uri, &tiny);
        assert!(
            matches!(e.kind, UrlErrorKind::LimitExceeded { limit, .. } if limit == kind),
            "{uri}: {e}"
        );
        e.offset
    };
    assert_eq!(
        limit("Buildings('b1')/address/city", LimitKind::PathLength),
        20
    );
    assert_eq!(
        limit(
            "Buildings?$filter=name eq 'abcdefghijklmnopq'",
            LimitKind::QueryLength
        ),
        40
    );
    limit("Sensors(1)/owner", LimitKind::PathSegments);
    limit("Sensors?$top=1&$skip=1&a=1", LimitKind::QueryOptions);
    limit("Sensors?$filter=(((true)))", LimitKind::ExprDepth);
    limit(
        "Sensors?$filter=value eq 1 or at eq 2",
        LimitKind::ExprNodes,
    );
    limit("Sensors?$expand=owner($expand=x)", LimitKind::ExpandDepth);
    limit("Sensors?$expand=owner,owner,owner", LimitKind::ExpandItems);
    limit("Sensors?$select=value,at,value", LimitKind::SelectItems);
    limit("Sensors?$orderby=value,at,value", LimitKind::OrderbyItems);
    limit("Sensors?$filter=value in (1,2,3)", LimitKind::InListLength);
    limit("Sensors?$filter='abcdefgh'", LimitKind::LiteralLength);
    let fits = parse("Sensors?$filter=value in (1,2)", &Service, &tiny);
    assert!(fits.is_ok(), "{fits:?}");
}

#[test]
fn normalisation() {
    assert_eq!(normalize("Buildings"), "Buildings");
    assert_eq!(normalize("%42uildings%2F"), "Buildings%2F");
    assert_eq!(normalize("%7e%2d%5F%2E%30"), "~-_.0");
    // A decoded hex digit never spells a new escape.
    assert_eq!(normalize("%6%65"), "%6%65");
    assert_eq!(normalize("%%36%35"), "%%365");
    assert_eq!(normalize("%25%36%35"), "%2565");
    assert_eq!(normalize("%%6G%41"), "%%6GA");
    assert_eq!(normalize("\u{e9}%41"), "\u{e9}A");
    assert_eq!(normalize("%4"), "%4");
    refused("B%C3%A4ume", Feature::NonAsciiIdentifier, 1);
}

/// F-R2ANT4-016: an `&` inside a string literal starts no option, so the
/// `$apply` pre-scan does not read one out of a filter's text.
#[test]
fn an_ampersand_in_a_literal_starts_no_option() {
    for uri in [
        "Buildings?$filter=name%20eq%20'x&$apply=aggregate($count%20as%20n)'",
        "Buildings?$filter=name%20eq%20%27x&$apply=aggregate($count%20as%20n)%27",
    ] {
        let o = options(uri);
        assert!(o.apply.is_none(), "{uri}: {:?}", o.apply);
        assert!(o.filter.is_some(), "{uri}");
    }
}

/// F-R2ANT4-015: an offset is into the URL as given, so a name after decoded
/// escapes is placed past the path, not inside it (odata-axum answers 404 or
/// 400 by that offset).
#[test]
fn an_offset_counts_the_escapes_before_it() {
    let plain =
        parse("Sensors?$filter=nope eq 1", &Service, &Limits::strict()).expect_err("unknown name");
    let escaped = parse("%53%65nsors?$filter=nope eq 1", &Service, &Limits::strict())
        .expect_err("unknown name");
    assert_eq!(escaped.kind, plain.kind);
    assert_eq!(plain.offset, 16);
    assert_eq!(escaped.offset, 20, "{escaped:?}");
}

#[test]
fn a_form_encoded_query_reads_as_written_plain() {
    // T-3719: `%24` names a system option, `+` is whitespace where `RWS` stands, `%2F` and
    // `%3D` are the query's `/` and `=`.
    let o = options("Buildings?%24top=1&%24Skip=2&%24filter=height+gt+5&%24orderby=height+desc");
    assert_eq!((o.top, o.skip), (Some(1), Some(2)));
    assert_eq!(o.filter, Some(filter("height gt 5")));
    assert_eq!(o.orderby, options("Buildings?$orderby=height desc").orderby);
    assert!(o.custom.is_empty(), "{:?}", o.custom);
    assert_eq!(
        filter("height+gt+5+and+name+eq+%27x%27"),
        filter("height gt 5 and name eq 'x'")
    );
    let sensors = options("Buildings?$expand=sensors(%24select%3Dvalue)").expand;
    assert_eq!(
        sensors,
        options("Buildings?$expand=sensors($select=value)").expand
    );
    assert_eq!(
        filter("address%2Fcity+eq+%27Bonn%27"),
        filter("address/city eq 'Bonn'")
    );
    let o = options("Buildings?%24select=address%2Fcity&%24expand=address%2Fsensors");
    assert_eq!((o.select, o.expand), {
        let plain = options("Buildings?$select=address/city&$expand=address/sensors");
        (plain.select, plain.expand)
    });
    assert_eq!(
        options("Buildings?@h%3D10").aliases,
        vec![("h".to_owned(), Literal::Int(10))]
    );
    // A custom option keeps its `=` as written, and a key keeps its `%2F`.
    assert_eq!(
        options("Buildings?mode%3Dfast").custom,
        vec![("mode%3Dfast".to_owned(), None)]
    );
    let key = ok("Buildings('a%2Fb')").to_string();
    assert!(key.contains("a%2Fb"), "{key}");
    // Encoded twice is a custom option; a duplicate across both spellings is one.
    assert_eq!(
        options("Buildings?%2524top=1").custom,
        vec![("%2524top".to_owned(), Some("1".to_owned()))]
    );
    let dup = err("Buildings?%24top=1&$top=2");
    assert_eq!(dup.kind, UrlErrorKind::DuplicateOption("$top"));
    // Where `RWS` stands a `+` could only be a sign, which means nothing.
    assert_eq!(filter("height gt +5"), filter("height gt 5"));
    assert_eq!(filter("height+gt+%2B5"), filter("height gt 5"));
    assert_eq!(filter("height+gt+-5"), filter("height gt -5"));
    // A literal keeps its `+`, and a decoded `&` or `=` never splits an option.
    assert_eq!(
        filter("name+eq+'a+b'"),
        Expr::Compare(
            Box::new(member(&["name"])),
            CompareOp::Eq,
            Box::new(Expr::Literal(string("a+b")))
        )
    );
    let o = options("Buildings?$filter=name+eq+'a%26top%3D1'");
    assert_eq!(o.top, None);
    assert_eq!(o.filter, Some(filter("name eq 'a&top=1'")));
    // `BWS` is not `RWS`: a `+` after a comma is no whitespace.
    syntax("Buildings?$select=name,+height", 23);
    // No whitespace after `gt` is still none: the error stands at the `5`.
    syntax("Buildings?$filter=height+gt5", 27);
}

#[test]
fn the_canonical_form_parses_back() {
    for uri in [
        "",
        "$metadata?$format=json",
        "$batch?$format=xml",
        "$entity?$id=Buildings('b1')&$format=atom",
        "Buildings('b1')/address/city",
        "Buildings(id='b%20''1',height=@h)/sensors(1)/owner/$ref?@h=2",
        "Buildings/$count",
        "Buildings('x')/name/$value",
        "Buildings/$query",
        "Buildings?$filter=(height gt 1 or height lt 2) and not (name eq 'a%23b') and name in ('x','y')",
        "Buildings?$filter=height gt -1.5e3 and built lt 2024-01-01T00:00:00%2B01:00 and built ge 2024-01-01",
        "Buildings?$filter=not (not true) or (height ne 1 and false) or (height eq 1) eq true",
        "Buildings?$filter=height le 1 eq (height ge 2)",
        "Buildings?$filter=contains(name,'%C3%A9') and startswith(name,'a') and endswith(name,'z')",
        "Buildings?$filter=geo.intersects(location,geography'SRID=4326;Polygon((0 0,1 0,1 1,0 0))')",
        "Buildings?$filter=geo.intersects(location,geography'SRID=0;Point(1 2)')",
        "Buildings?$filter=rooms/any(r:r/area gt 1) and tags/any() and @p and null eq 10:00",
        "Buildings?$filter=name/@Core.Size%23max eq 1",
        "Buildings?$select=*,name,address/city,@Core.Messages&$orderby=height desc,name",
        "Buildings?$expand=sensors($select=value;$expand=owner;$levels=1),owner/$ref,*($levels=max),*/$ref",
        "Buildings?$top=1&$skip=2&$count=true&$format=application/json&$skiptoken=x&$schemaversion=*&mode=a&flag",
    ] {
        let first = ok(uri);
        let canonical = first.to_string();
        assert_eq!(ok(&canonical), first, "{uri} wrote {canonical}");
        assert_eq!(ok(&canonical).to_string(), canonical, "{uri}");
    }
    assert_eq!(
        ok("Buildings?$filter=height gt 1 and (height lt 2 or name eq 'it''s')").to_string(),
        "Buildings?$filter=height%20gt%201%20and%20(height%20lt%202%20or%20name%20eq%20'it''s')"
    );
    assert_eq!(
        ok("Buildings?$filter=name eq 'a%20b%23'").to_string(),
        "Buildings?$filter=name%20eq%20'a%20b%23'"
    );
}

#[test]
fn errors_name_themselves() {
    let e = err("Buildings?$index=1");
    assert_eq!(e.to_string(), "not implemented at byte 10: $index");
    let e = err("Towers");
    assert_eq!(e.to_string(), "unknown name at byte 0");
    let e = err("Buildings?$top=1&$top=1");
    assert_eq!(e.to_string(), "system query option $top given twice");
    let e = err("Buildings?$top=x");
    assert_eq!(e.to_string(), "syntax error at byte 15: expected DIGIT");
    let e = err_within(
        "Buildings",
        &Limits {
            max_path_len: 1,
            ..Limits::strict()
        },
    );
    assert_eq!(
        e.to_string(),
        format!(
            "limit exceeded at byte 1: {} (max 1)",
            LimitKind::PathLength.as_str()
        )
    );
}

#[test]
fn every_feature_and_limit_has_its_own_name() {
    use Feature as F;
    use LimitKind as L;
    let features = [
        F::Index,
        F::Apply,
        F::DeltaToken,
        F::Root,
        F::ImplicitVariable,
        F::AliasExpression,
        F::FilterSegment,
        F::CrossJoin,
        F::TypeCast,
        F::Ordinal,
        F::Operation,
        F::Each,
        F::MediaValue,
        F::NestedOption,
        F::NestedCount,
        F::PathInExpression,
        F::Has,
        F::Method,
        F::CastIsOf,
        F::InExpression,
        F::JsonLiteral,
        F::Literal,
        F::NonAsciiIdentifier,
        F::AnnotationPath,
        F::PathStar,
    ];
    let limits = [
        L::PathLength,
        L::QueryLength,
        L::PathSegments,
        L::QueryOptions,
        L::ExprDepth,
        L::ExprNodes,
        L::ExpandDepth,
        L::ExpandItems,
        L::SelectItems,
        L::OrderbyItems,
        L::InListLength,
        L::LiteralLength,
        L::Number,
    ];
    let mut names: Vec<&str> = features
        .iter()
        .map(|f| f.as_str())
        .chain(limits.iter().map(|l| l.as_str()))
        .collect();
    let all = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), all);
    assert!(!names.contains(&""));
}

#[test]
fn every_option_names_itself_when_given_twice() {
    for (option, value) in [
        ("$filter", "true"),
        ("$select", "name"),
        ("$expand", "owner"),
        ("$orderby", "name"),
        ("$top", "1"),
        ("$skip", "1"),
        ("$count", "true"),
        ("$format", "json"),
        ("$skiptoken", "x"),
        ("$schemaversion", "1"),
        ("$id", "x"),
    ] {
        let e = err(&format!("Buildings?{option}={value}&{option}={value}"));
        assert_eq!(e.kind, UrlErrorKind::DuplicateOption(option), "{option}");
    }
}

#[test]
fn the_rest_of_the_grammar() {
    // Signs and whitespace in their encoded forms.
    assert_eq!(
        filter("height%09gt%09%2B1"),
        Expr::Compare(
            Box::new(member(&["height"])),
            CompareOp::Gt,
            Box::new(int(1))
        )
    );
    let base = "Buildings?$filter=".len();
    for (text, feature, offset) in [
        ("%5B1]", Feature::JsonLiteral, 0),
        ("%7B}", Feature::JsonLiteral, 0),
        ("height has Model.Kind'a'", Feature::Has, 7),
    ] {
        refused(
            &format!("Buildings?$filter={text}"),
            feature,
            base.saturating_add(offset),
        );
    }
    for name in ["contains", "length", "cast", "nothing"] {
        unresolved(&format!("Buildings?$filter={name}"), base);
    }
    // `not` needs whitespace: `not(` is a call of an unknown function.
    syntax("Buildings?$filter=not(true)", base.saturating_add(3));
    unresolved(
        "Buildings?$filter=tags/any(t:t/x eq 1)",
        base.saturating_add(13),
    );
    syntax("Buildings?$filter=name/1", base.saturating_add(5));
    syntax("Buildings?$filter=tags/any", base.saturating_add(8));
    syntax("Buildings?$filter=tags/all", base.saturating_add(8));
    syntax("Buildings?$filter=tags/1", base.saturating_add(5));
    assert_eq!(filter("address"), member(&["address"]));
    assert_eq!(
        filter("owner eq null"),
        Expr::Compare(
            Box::new(member(&["owner"])),
            CompareOp::Eq,
            Box::new(Expr::Literal(Literal::Null))
        )
    );
    syntax("Buildings?$orderby=name x", 24);
    unresolved("Buildings('b1')/name?$select=x", 29);
    assert_eq!(
        options("Buildings?$format=a%2Fb/c").format,
        Some(Format::MediaType("a%2Fb/c".into()))
    );
    assert_eq!(
        options("Buildings?$format=jsonx/y").format,
        Some(Format::MediaType("jsonx/y".into()))
    );
    // T-3742: Go's url.QueryEscape writes the separator as %2F.
    assert_eq!(
        options("Buildings?$format=application%2Fjson%3Bodata.metadata%3Dnone").format,
        Some(Format::MediaType(
            "application%2Fjson%3Bodata.metadata%3Dnone".into()
        ))
    );
    syntax("Buildings?$format=/x", 18);
    syntax("Buildings?$format=abc", 21);
    syntax("Buildings?$format=%2Fx", 22);
    syntax("Buildings?$format=a%2F", 22);
    assert_eq!(filter("height\tgt 1"), filter("height gt 1"));
    syntax("Buildings?%zz", 10);
    syntax("Buildings?ab#", 12);
    let rings = ok("Buildings?$filter=geo.intersects(location,geography'SRID=0;Polygon((0 0,1 0,0 0),(0 0,2 0,0 0))')");
    assert_eq!(ok(&rings.to_string()), rings);
    let punctuated = ok("Buildings?$filter=name eq 'a-b.c_d~e!f(g)h*i,j;k$l&m=n:o@p+q'");
    assert_eq!(ok(&punctuated.to_string()), punctuated);
}

/// The offset just past `text` in a `$filter` of `Buildings`.
fn past(text: &str) -> usize {
    "Buildings?$filter=".len().saturating_add(text.len())
}

/// T-3749: arithmetic binds tighter than comparison, `mul div divby mod` tighter than `add sub`,
/// both left-associative, and negation tightest; a `-` before a number is the number.
#[test]
fn arithmetic_follows_the_precedence_of_5_1_1_16() {
    let arith = |l, op, r| Expr::Arith(Box::new(l), op, Box::new(r));
    assert_eq!(
        filter("height add 2 mul 3 gt 10"),
        Expr::Compare(
            Box::new(arith(
                member(&["height"]),
                ArithOp::Add,
                arith(int(2), ArithOp::Mul, int(3))
            )),
            CompareOp::Gt,
            Box::new(int(10)),
        )
    );
    assert_eq!(
        filter("height sub 1 sub 2 eq 0"),
        Expr::Compare(
            Box::new(arith(
                arith(member(&["height"]), ArithOp::Sub, int(1)),
                ArithOp::Sub,
                int(2)
            )),
            CompareOp::Eq,
            Box::new(int(0)),
        )
    );
    for (word, op) in [
        ("div", ArithOp::Div),
        ("divby", ArithOp::DivBy),
        ("mod", ArithOp::Mod),
    ] {
        assert_eq!(
            filter(&format!("height {word} 2 eq 1")),
            Expr::Compare(
                Box::new(arith(member(&["height"]), op, int(2))),
                CompareOp::Eq,
                Box::new(int(1)),
            ),
            "{word}"
        );
    }
    assert_eq!(
        filter("-height lt -2"),
        Expr::Compare(
            Box::new(Expr::Neg(Box::new(member(&["height"])))),
            CompareOp::Lt,
            Box::new(int(-2)),
        )
    );
    assert_eq!(
        filter("- (height add 1) lt 0"),
        Expr::Compare(
            Box::new(Expr::Neg(Box::new(arith(
                member(&["height"]),
                ArithOp::Add,
                int(1)
            )))),
            CompareOp::Lt,
            Box::new(int(0)),
        )
    );
    // A dangling operator is a syntax error at the operand it lacks.
    syntax("Buildings?$filter=height add", past("height add"));
}

/// T-3749: every method of the table parses with its arity; one argument too few fails where
/// the `COMMA` was due, one too many where the `CLOSE` was.
#[test]
fn methods_parse_with_their_arity() {
    for method in Method::all() {
        let args = match method.arity().0 {
            0 => String::new(),
            1 => "name".into(),
            _ => "name,name".into(),
        };
        // a method's name with no `(` after it is a member's name, here an unknown one; a
        // dotted one reads as a qualified cast
        let bare = err(&format!("Buildings?$filter={}%20eq%201", method.name()));
        let want = if method.name().contains('.') {
            "not implemented at byte 18: type cast or bound operation"
        } else {
            "unknown name at byte 18"
        };
        assert_eq!(bare.to_string(), want, "{method:?}");
        let text = format!("{}({args}) eq null", method.name().to_uppercase());
        let parsed = filter(&text);
        let Expr::Compare(call, CompareOp::Eq, _) = parsed else {
            panic!("{text}: {parsed:?}")
        };
        assert!(
            matches!(call.as_ref(), Expr::Call(m, a) if *m == method && a.len() == method.arity().0),
            "{text}: {call:?}"
        );
    }
    // so is the bare name of a method that stays 501, and of cast and isof
    for name in ["totalseconds", "cast", "isof"] {
        let bare = err(&format!("Buildings?$filter={name}%20eq%201"));
        assert_eq!(bare.to_string(), "unknown name at byte 18", "{name}");
    }
    assert_eq!(
        filter("substring(name,1,2) eq 'x'"),
        Expr::Compare(
            Box::new(Expr::Call(
                Method::Substring,
                vec![member(&["name"]), int(1), int(2)]
            )),
            CompareOp::Eq,
            Box::new(Expr::Literal(Literal::String("x".into()))),
        )
    );
    assert_eq!(
        filter("year(now()) gt 2000"),
        Expr::Compare(
            Box::new(Expr::Call(
                Method::Year,
                vec![Expr::Call(Method::Now, Vec::new())]
            )),
            CompareOp::Gt,
            Box::new(int(2000)),
        )
    );
    syntax(
        "Buildings?$filter=length(name,name) eq 1",
        past("length(name"),
    );
    syntax("Buildings?$filter=now(name) eq 1", past("now("));
    syntax("Buildings?$filter=concat(name) eq 'x'", past("concat(name"));
    syntax(
        "Buildings?$filter=substring(name,1,2,3) eq 'x'",
        past("substring(name,1,2"),
    );
    // A member that shares a method's name is still the member.
    assert_eq!(
        filter("built eq 1"),
        Expr::Compare(
            Box::new(member(&["built"])),
            CompareOp::Eq,
            Box::new(int(1))
        )
    );
}

/// T-3749: `all` reads like `any`, needs its lambda, and its variable is in scope in the body
/// only.
#[test]
fn all_lambdas_parse() {
    let area = Expr::Member(MemberPath {
        variable: Some("r".into()),
        path: vec!["area".into()],
    });
    assert_eq!(
        filter("rooms/all(r:r/area gt 10)"),
        Expr::All {
            collection: MemberPath {
                variable: None,
                path: vec!["rooms".into()],
            },
            lambda: (
                "r".into(),
                Box::new(Expr::Compare(
                    Box::new(area),
                    CompareOp::Gt,
                    Box::new(int(10))
                ))
            ),
        }
    );
    assert!(matches!(
        filter("tags/all(t:t ne 'x') and rooms/any()"),
        Expr::And(..)
    ));
    // past the `)`, as the OASIS runner reports `Products/all()`
    syntax("Buildings?$filter=tags/all()", past("tags/all()"));
    unresolved(
        "Buildings?$filter=tags/all(t:t eq 'x') and t eq 'x'",
        past("tags/all(t:t eq 'x') and "),
    );
}

/// T-3749: the canonical writer parenthesises only what the precedence needs, and what it
/// writes parses back to the same tree.
#[test]
fn arithmetic_and_lambdas_round_trip() {
    for text in [
        "Buildings?$filter=height%20add%202%20mul%203%20gt%2010",
        "Buildings?$filter=(height%20add%202)%20mul%203%20gt%2010",
        "Buildings?$filter=height%20sub%20(1%20sub%202)%20eq%200",
        "Buildings?$filter=-height%20lt%20-2",
        "Buildings?$filter=-(height%20add%201)%20lt%200",
        "Buildings?$filter=-(-1)%20eq%201",
        "Buildings?$filter=not%20(height%20add%201%20gt%202)",
        "Buildings?$filter=rooms/all(r:r/area%20gt%2010)",
        "Buildings?$filter=substring(name,1,2)%20eq%20'x'",
        "Buildings?$filter=round(height%20divby%203)%20eq%201",
        "Buildings?$filter=matchesPattern(name,'%5EA.*e$')",
    ] {
        let canonical = ok(text).to_string();
        assert_eq!(canonical, text, "{text}");
        assert_eq!(ok(&canonical).to_string(), canonical, "{text}");
    }
}

/// Fuzz run 38677615890 (`odata_url_parse`): `$search=OR` read as a word wrote itself `"OR"`, a
/// phrase, so the canonical URL parsed to another tree.
#[test]
fn search_operator_read_as_text_round_trips() {
    for query in [
        "$search=OR",
        "$search=AND",
        "$search=NOT",
        "$search=a%20OR",
        "$search=(OR)",
    ] {
        let text = format!("Buildings?{query}");
        let first = ok(&text);
        let canonical = first.to_string();
        assert_eq!(ok(&canonical), first, "{text} wrote {canonical}");
    }
    assert_eq!(search_of("$search=OR"), Search::Phrase("OR".into()));
    // a word built by hand as an operator writes as the phrase it parses back to
    assert_eq!(Search::Word("NOT".into()).to_string(), "%22NOT%22");
}

fn search_of(query: &str) -> Search {
    options(&format!("Buildings?{query}"))
        .search
        .unwrap_or(Search::Word(String::new()))
}

#[test]
fn search_reads_words_phrases_and_operators_with_their_precedence() {
    use Search::{And, Not, Or, Phrase, Word};
    let word = |w: &str| Box::new(Word(w.into()));
    assert_eq!(search_of("$search=blue"), Word("blue".into()));
    assert_eq!(search_of("search=blue"), Word("blue".into()));
    // side by side is AND; NOT binds tighter than AND, AND tighter than OR (URL 5.1.7)
    assert_eq!(search_of("$search=a b"), And(word("a"), word("b")));
    assert_eq!(search_of("$search=a%20AND%20b"), And(word("a"), word("b")));
    assert_eq!(
        search_of("$search=a b OR c"),
        Or(Box::new(And(word("a"), word("b"))), word("c"))
    );
    assert_eq!(
        search_of("$search=a OR b c"),
        Or(word("a"), Box::new(And(word("b"), word("c"))))
    );
    assert_eq!(
        search_of("$search=NOT a b"),
        And(Box::new(Not(word("a"))), word("b"))
    );
    assert_eq!(
        search_of("$search=NOT (a OR b)"),
        Not(Box::new(Or(word("a"), word("b"))))
    );
    assert_eq!(
        search_of("$search=( a OR b ) c"),
        And(Box::new(Or(word("a"), word("b"))), word("c"))
    );
    // a form encoder's `+` is a space; a plus in a word is written `%2B`
    assert_eq!(search_of("$search=a+OR+b"), Or(word("a"), word("b")));
    assert_eq!(search_of("$search=C%2B%2B"), Word("C++".into()));
    // operators are upper case and need whitespace on both sides; otherwise they are words
    assert_eq!(
        search_of("$search=a or b"),
        And(Box::new(And(word("a"), word("or"))), word("b"))
    );
    // an operator where it cannot operate is searched for as text, a phrase
    assert_eq!(search_of("$search=NOT"), Phrase("NOT".into()));
    assert_eq!(search_of("$search=NOTE"), Word("NOTE".into()));
    assert_eq!(
        search_of("$search=a OR"),
        And(word("a"), Box::new(Phrase("OR".into())))
    );
    // phrases keep spaces, parentheses and percent-decoded text
    assert_eq!(
        search_of("$search=\"dark red\" OR %22a (b)%22"),
        Or(
            Box::new(Phrase("dark red".into())),
            Box::new(Phrase("a (b)".into()))
        )
    );
    // `&` and `#` only percent-encoded; UTF-8 decoded; a quote inside a word
    assert_eq!(search_of("$search=R%26D%23"), Word("R&D#".into()));
    assert_eq!(search_of("$search=caf%C3%A9"), Word("café".into()));
    assert_eq!(search_of("$search=it's"), Word("it's".into()));
    // a literal `;` ends a word, a percent-encoded one is text (ABNF `searchWord`)
    assert_eq!(search_of("$search=a%3Bb"), Word("a;b".into()));
    syntax("Buildings?$search=a;b", "Buildings?$search=a".len());
    // `searchExpr-incomplete` is one phrase, `''` a quote
    assert_eq!(search_of("$search='it''s red'"), Phrase("it's red".into()));
    // `+` is a space in a phrase too; `&` percent-encoded is text
    assert_eq!(search_of("$search=%22a+b%22"), Phrase("a b".into()));
    assert_eq!(search_of("$search='a+b%26c'"), Phrase("a b&c".into()));
    // whitespace before the end of the option or the next one
    assert_eq!(search_of("$search=a%20"), Word("a".into()));
    let spaced = options("Buildings?$search=a%20&$top=1");
    assert_eq!(
        (spaced.search, spaced.top),
        (Some(Word("a".into())), Some(1))
    );
    // the option ends at `&`
    let both = options("Buildings?$search=a&$top=1");
    assert_eq!((both.search, both.top), (Some(Word("a".into())), Some(1)));
}

#[test]
fn search_refuses_what_is_no_search_expression() {
    for query in [
        "$search=",
        "$search=%20",
        "$search=(a",
        "$search=a)",
        "$search=()",
        "$search=\"\"",
        "$search=\"%20\"",
        "$search=\"open",
        "$search='open",
        "$search=''",
        "$search='x'y",
        "$search=a%2",
        "$search=%FF",
        "$search=a;b",
        "$search=a 'b",
    ] {
        let e = err(&format!("Buildings?{query}"));
        assert!(
            matches!(e.kind, UrlErrorKind::Syntax { .. }),
            "{query}: {e:?}"
        );
    }
    let dup = err("Buildings?$search=a&search=b");
    assert_eq!(dup.to_string(), "system query option $search given twice");
}

#[test]
fn search_is_bounded() {
    let tight = Limits {
        max_expr_depth: 2,
        max_expr_nodes: 5,
        max_literal_len: 4,
        ..Limits::strict()
    };
    let limit = |q: &str| {
        let e = err_within(&format!("Buildings?{q}"), &tight);
        match e.kind {
            UrlErrorKind::LimitExceeded { limit, .. } => limit,
            other => panic!("{q}: {other:?}"),
        }
    };
    assert_eq!(limit("$search=a b c d"), LimitKind::ExprNodes);
    assert_eq!(limit("$search=NOT NOT NOT a"), LimitKind::ExprDepth);
    assert_eq!(limit("$search=(((a)))"), LimitKind::ExprDepth);
    assert_eq!(limit("$search=abcdef"), LimitKind::LiteralLength);
    assert_eq!(limit("$search=\"abcdef\""), LimitKind::LiteralLength);
    assert_eq!(limit("$search='abcdef'"), LimitKind::LiteralLength);
    assert!(parse("Buildings?$search=NOT (a)", &Service, &tight).is_ok());
}

#[test]
fn search_writes_its_canonical_form() {
    for (query, written) in [
        ("$search=a b OR c", "$search=a%20AND%20b%20OR%20c"),
        ("$search=a (b OR c)", "$search=a%20AND%20(b%20OR%20c)"),
        ("$search=NOT (a b)", "$search=NOT%20(a%20AND%20b)"),
        ("$search=\"x y\"", "$search=%22x%20y%22"),
        ("$search=C%2B%2B", "$search=C%2B%2B"),
        ("$search=a OR", "$search=a%20AND%20%22OR%22"),
        // T-4205: a quote inside `'...'` is no `searchPhrase`; it writes back as it was read
        ("$search='\"'", "$search=%27%22%27"),
        ("$search='it''s \"x\"'", "$search=%27it%27%27s%20%22x%22%27"),
    ] {
        let uri = ok(&format!("Buildings?{query}"));
        assert_eq!(uri.to_string(), format!("Buildings?{written}"), "{query}");
        assert_eq!(ok(&uri.to_string()).to_string(), uri.to_string(), "{query}");
    }
}

fn compute_of(query: &str) -> Vec<Compute> {
    options(&format!("Buildings?{query}"))
        .compute
        .unwrap_or_default()
}

fn computed(expr: Expr, alias: &str) -> Compute {
    Compute {
        expr,
        alias: alias.to_owned(),
    }
}

#[test]
fn compute_reads_items_and_its_names_resolve_in_the_other_options() {
    let double = Expr::Arith(
        Box::new(member(&["height"])),
        ArithOp::Mul,
        Box::new(int(2)),
    );
    assert_eq!(
        compute_of("$compute=height mul 2 as double"),
        [computed(double.clone(), "double")]
    );
    assert_eq!(
        compute_of("compute=height mul 2 as double,tolower(name) AS low"),
        [
            computed(double, "double"),
            computed(Expr::Call(Method::ToLower, vec![member(&["name"])]), "low"),
        ]
    );
    // the names resolve in $filter, $orderby and $select, wherever $compute stands
    let o = options(
        "Buildings?$filter=double gt 10 and tags/any(t: t eq low)&$orderby=double desc\
         &$select=name,double&$compute=height mul 2 as double,name as low",
    );
    assert_eq!(
        o.filter,
        Some(Expr::And(
            Box::new(Expr::Compare(
                Box::new(member(&["double"])),
                CompareOp::Gt,
                Box::new(int(10))
            )),
            Box::new(Expr::Any {
                collection: MemberPath {
                    variable: None,
                    path: vec!["tags".into()]
                },
                lambda: Some((
                    "t".into(),
                    Box::new(Expr::Compare(
                        Box::new(Expr::Member(MemberPath {
                            variable: Some("t".into()),
                            path: vec![]
                        })),
                        CompareOp::Eq,
                        Box::new(member(&["low"]))
                    ))
                ))
            })
        ))
    );
    assert_eq!(
        o.orderby,
        Some(vec![OrderBy {
            expr: member(&["double"]),
            descending: true
        }])
    );
    assert_eq!(
        o.select,
        Some(vec![
            SelectItem::Path(vec!["name".into()]),
            SelectItem::Path(vec!["double".into()])
        ])
    );
    assert_eq!(o.compute.map(|c| c.len()), Some(2));
}

#[test]
fn compute_refuses_what_is_no_compute_item() {
    let at = |q: &str| format!("Buildings?{q}").len();
    // no `as`, no name, a name the type has, a name given twice, junk after the item
    syntax("Buildings?$compute=height", at("$compute=height"));
    syntax("Buildings?$compute=height as", at("$compute=height as"));
    syntax("Buildings?$compute=height is h", at("$compute=height "));
    syntax("Buildings?$compute=height asx", at("$compute=height as"));
    syntax("Buildings?$compute=height as 1x", at("$compute=height as "));
    syntax(
        "Buildings?$compute=height as name",
        at("$compute=height as name"),
    );
    syntax(
        "Buildings?$compute=height as h,name as h",
        at("$compute=height as h,name as h"),
    );
    syntax(
        "Buildings?$compute=height as h)",
        at("$compute=height as h"),
    );
    // a computed name is the resource's: not past a path, not a lambda variable's member,
    // not in a nested $select, not in an item beside it, not without $compute
    unresolved(
        "Buildings?$compute=height as h&$filter=address/h eq 1",
        at("$compute=height as h&$filter=address/"),
    );
    unresolved(
        "Buildings?$compute=height as h&$filter=rooms/any(r: r/h eq 1)",
        at("$compute=height as h&$filter=rooms/any(r: r/"),
    );
    unresolved(
        "Buildings?$compute=height as h&$expand=owner($select=h)",
        at("$compute=height as h&$expand=owner($select="),
    );
    unresolved(
        "Buildings?$compute=height as h,h as g",
        at("$compute=height as h,"),
    );
    unresolved("Buildings?$filter=h eq 1", at("$filter="));
    let dup = err("Buildings?$compute=height as h&compute=height as g");
    assert_eq!(dup.to_string(), "system query option $compute given twice");
    // nested in $expand it stays refused
    refused(
        "Buildings?$expand=owner($compute=1 as x)",
        Feature::NestedOption,
        at("$expand=owner("),
    );
    // and outside the query forms with every option
    syntax("$batch?$compute=1 as x", "$batch?".len());
}

#[test]
fn compute_is_bounded_by_the_expression_budget() {
    let tight = Limits {
        max_expr_nodes: 3,
        ..Limits::strict()
    };
    let e = err_within(
        "Buildings?$compute=height add 1 as a,height add 1 as b",
        &tight,
    );
    assert!(
        matches!(
            e.kind,
            UrlErrorKind::LimitExceeded {
                limit: LimitKind::ExprNodes,
                ..
            }
        ),
        "{e:?}"
    );
    assert!(parse("Buildings?$compute=height add 1 as a", &Service, &tight).is_ok());
}

#[test]
fn compute_writes_its_canonical_form() {
    for (query, written) in [
        (
            "$filter=d gt 1&$compute=height mul 2 as d",
            "$compute=height%20mul%202%20as%20d&$filter=d%20gt%201",
        ),
        (
            "compute=name as n,height as h&$select=n",
            "$compute=name%20as%20n,height%20as%20h&$select=n",
        ),
    ] {
        let uri = ok(&format!("Buildings?{query}"));
        assert_eq!(uri.to_string(), format!("Buildings?{written}"), "{query}");
        assert_eq!(ok(&uri.to_string()).to_string(), uri.to_string(), "{query}");
    }
}

fn apply_of(query: &str) -> Vec<Transformation> {
    options(&format!("Buildings?$apply={query}"))
        .apply
        .unwrap_or_default()
}

fn name(n: &str) -> Expr {
    Expr::Member(MemberPath {
        variable: None,
        path: vec![n.into()],
    })
}

#[test]
fn apply_reads_each_served_transformation() {
    let height = name("height");
    assert_eq!(
        apply_of("identity/filter(height%20gt%201)/orderby(name%20desc,height)/top(2)/skip(1)"),
        vec![
            Transformation::Identity,
            Transformation::Filter(Expr::Compare(
                Box::new(height.clone()),
                CompareOp::Gt,
                Box::new(Expr::Literal(Literal::Int(1)))
            )),
            Transformation::OrderBy(vec![
                OrderBy {
                    expr: name("name"),
                    descending: true
                },
                OrderBy {
                    expr: height.clone(),
                    descending: false
                }
            ]),
            Transformation::Top(2),
            Transformation::Skip(1),
        ]
    );
    let sum = |with, alias: &str| Aggregate::With {
        expr: height.clone(),
        with,
        alias: alias.into(),
    };
    assert_eq!(
        apply_of(
            "aggregate(height with sum as s, height with MIN as lo,height with max as hi,\
             height with average as avg,height with countdistinct as d,$count as n)"
        ),
        vec![Transformation::Aggregate(vec![
            sum(AggregateWith::Sum, "s"),
            sum(AggregateWith::Min, "lo"),
            sum(AggregateWith::Max, "hi"),
            sum(AggregateWith::Average, "avg"),
            sum(AggregateWith::CountDistinct, "d"),
            Aggregate::Count { alias: "n".into() },
        ])]
    );
    assert_eq!(
        apply_of("groupby( (name, built) , aggregate( $count as n ) )"),
        vec![Transformation::GroupBy {
            properties: vec!["name".into(), "built".into()],
            aggregate: Some(vec![Aggregate::Count { alias: "n".into() }]),
        }]
    );
    assert_eq!(
        apply_of("groupby((name))"),
        vec![Transformation::GroupBy {
            properties: vec!["name".into()],
            aggregate: None,
        }]
    );
}

#[test]
fn after_an_aggregation_only_its_names_resolve() {
    // in the later transformations and in the other options, wherever `$apply` stands
    let o = options(
        "Buildings?$filter=n%20gt%201&$apply=groupby((name),aggregate($count%20as%20n))\
         /filter(n%20gt%200)/orderby(name)&$orderby=n&$select=name&$compute=n%20add%201%20as%20m",
    );
    assert_eq!(o.apply.map(|a| a.len()), Some(3));
    assert_eq!(o.compute.map(|c| c.len()), Some(1));
    // a member of the entities no longer resolves, nor the other's names
    unresolved(
        "Buildings?$apply=aggregate($count as n)&$filter=height gt 1",
        48,
    );
    unresolved("Buildings?$apply=groupby((name))/filter(height gt 1)", 40);
    unresolved(
        "Buildings?$apply=aggregate($count as n)/groupby((name))",
        49,
    );
    unresolved("Buildings?$apply=groupby((nothing))", 26);
    // an aggregation over what an earlier one produced
    assert_eq!(
        apply_of("groupby((name),aggregate($count as n))/aggregate(n with sum as total)").len(),
        2
    );
    // a `$compute` name may not repeat one `$apply` produced
    syntax(
        "Buildings?$apply=aggregate($count as n)&$compute=1 add 1 as n",
        61,
    );
    // without an aggregation the entities' members stay
    assert!(options("Buildings?$apply=top(1)&$filter=height gt 1")
        .filter
        .is_some());
}

#[test]
fn apply_refuses_what_is_no_served_transformation() {
    for (query, offset) in [
        ("expand(sensors)", 17),
        ("concat(identity,aggregate($count as n))", 17),
        ("rollup(name)", 17),
        ("ns.custom(1)", 17),
        ("groupby((rollup(name)))", 26),
        ("groupby((address/city))", 26),
        ("groupby((address))", 26),
        ("groupby((tags))", 26),
        ("groupby((name.x))", 26),
        ("groupby((name),topcount(1,height))", 32),
        ("groupby((name),identity)", 32),
        ("aggregate(height)", 33),
        ("aggregate(height from name with sum as s)", 33),
        ("aggregate(height with ns.f as s)", 39),
        ("aggregate(height with)", 33),
    ] {
        refused(&format!("Buildings?$apply={query}"), Feature::Apply, offset);
    }
    for (query, offset) in [
        ("", 17),
        ("identity/", 26),
        ("unknown(1)", 25),
        ("filter", 23),
        ("top(x)", 21),
        ("top(1", 22),
        ("groupby(name)", 25),
        ("groupby(())", 26),
        ("groupby((name)", 31),
        ("groupby((name,name))", 35),
        ("groupby((name),aggregate)", 41),
        ("groupby((name),aggregate($count as n)", 54),
        ("groupby((name),aggregate($count as name))", 56),
        ("aggregate($count)", 33),
        ("groupby((name x))", 31),
        ("groupby((name),aggregate($count as n x))", 54),
        ("aggregate($count is n)", 34),
        ("concat(identity)", 32),
        ("concat(top(1),skip(1)", 38),
        ("topcount(1 height)", 28),
        ("topcount(1,height", 34),
        ("search(blue", 28),
        ("search()", 25),
        ("compute(height as name)", 39),
        ("compute(height as h)/compute(1 as h)", 52),
        ("aggregate($count as 1)", 37),
        ("aggregate($count as)", 36),
        ("aggregate(height with median as m)", 45),
        ("aggregate(height with sum as s,$count as s)", 59),
    ] {
        syntax(&format!("Buildings?$apply={query}"), offset);
    }
    // Protocol 4.01 11.2.1: names are case-insensitive, the `$` optional (T-5206 finding 17)
    for q in ["apply=identity", "$APPLY=identity", "Apply=identity"] {
        let o = options(&format!("Buildings?{q}"));
        assert!(o.apply.is_some() && o.custom.is_empty(), "{q}");
    }
    let e = err("Buildings?$apply=identity&$apply=identity");
    assert_eq!(e.kind, UrlErrorKind::DuplicateOption("$apply"));
}

#[test]
fn apply_writes_its_canonical_form() {
    for (query, canonical) in [
        (
            "identity/filter(height gt 1)/orderby(name desc)/top(2)/skip(1)",
            "identity/filter(height%20gt%201)/orderby(name%20desc)/top(2)/skip(1)",
        ),
        (
            "groupby((name,built),aggregate(height with average as a,$count as n))",
            "groupby((name,built),aggregate(height%20with%20average%20as%20a,$count%20as%20n))",
        ),
        ("groupby((name))", "groupby((name))"),
        (
            "topcount(2,height)/bottompercent(2.5,height)/search(a OR b)/compute(1 as c)",
            "topcount(2,height)/bottompercent(2.5,height)/search(a%20OR%20b)/compute(1%20as%20c)",
        ),
        (
            "concat(top(1)/identity,skip(2))",
            "concat(top(1)/identity,skip(2))",
        ),
        (
            "aggregate(height with sum as s,height with min as lo,height with max as hi,\
             height with countdistinct as d)",
            "aggregate(height%20with%20sum%20as%20s,height%20with%20min%20as%20lo,\
             height%20with%20max%20as%20hi,height%20with%20countdistinct%20as%20d)",
        ),
    ] {
        let uri = ok(&format!("Buildings?$apply={query}"));
        assert_eq!(uri.to_string(), format!("Buildings?$apply={canonical}"));
        assert_eq!(ok(&uri.to_string()), uri);
    }
}

#[test]
fn apply_reads_partitions_concat_compute_and_search() {
    let height = name("height");
    let partition = |by, top, n: Literal| Transformation::Partition {
        by,
        top,
        n: Expr::Literal(n),
        expr: height.clone(),
    };
    assert_eq!(
        apply_of(
            "topcount(2,height)/topsum(10.5,height)/toppercent(50,height)/\
             bottomcount(1,height)/bottomsum(3,height)/bottompercent(25,height)"
        ),
        vec![
            partition(PartitionBy::Count, true, Literal::Int(2)),
            partition(PartitionBy::Sum, true, Literal::Decimal("10.5".into())),
            partition(PartitionBy::Percent, true, Literal::Int(50)),
            partition(PartitionBy::Count, false, Literal::Int(1)),
            partition(PartitionBy::Sum, false, Literal::Int(3)),
            partition(PartitionBy::Percent, false, Literal::Int(25)),
        ]
    );
    assert_eq!(
        apply_of("search(blue OR red)/compute(height as h)"),
        vec![
            Transformation::Search(Search::Or(
                Box::new(Search::Word("blue".into())),
                Box::new(Search::Word("red".into()))
            )),
            Transformation::Compute(vec![Compute {
                expr: height.clone(),
                alias: "h".into()
            }]),
        ]
    );
    assert_eq!(
        apply_of("concat(top(1),skip(1)/identity)"),
        vec![Transformation::Concat(vec![
            vec![Transformation::Top(1)],
            vec![Transformation::Skip(1), Transformation::Identity],
        ])]
    );
    unresolved("Buildings?$apply=topcount(1,nothing)", 28);
}

#[test]
fn a_computed_name_and_the_names_a_concat_produces_resolve_after_them() {
    // a `compute` name in the later transformations and options, the members stay
    let o = options(
        "Buildings?$apply=compute(height%20mul%202%20as%20h)/filter(h%20gt%201)\
         &$filter=h%20gt%20height",
    );
    assert_eq!(o.apply.map(|a| a.len()), Some(2));
    // what every aggregating branch produced, and nothing else
    assert_eq!(
        apply_of(
            "concat(aggregate($count as n),groupby((name),aggregate($count as k)))\
             /filter(n gt 1 or k gt 1 or name eq 'x')"
        )
        .len(),
        2
    );
    // a name two branches produce resolves once
    assert_eq!(
        apply_of("concat(aggregate($count as n),aggregate($count as n))/filter(n gt 1)").len(),
        2
    );
    unresolved(
        "Buildings?$apply=concat(aggregate($count as n),groupby((name)))&$filter=height gt 1",
        72,
    );
    // branches that keep the entities keep their members, never a branch's computed name
    assert!(
        options("Buildings?$apply=concat(top(1),compute(1 as c))&$filter=height gt 1")
            .filter
            .is_some()
    );
    unresolved(
        "Buildings?$apply=concat(top(1),compute(1 as c))/filter(c eq 1)",
        55,
    );
    assert!(Transformation::Concat(vec![vec![Transformation::GroupBy {
        properties: Vec::new(),
        aggregate: None
    }]])
    .aggregates());
}

#[test]
fn apply_chains_and_nestings_cost_their_budget() {
    let long = Limits {
        max_query_len: 1 << 20,
        ..Limits::strict()
    };
    let limit_of = |uri: &str| match err_within(uri, &long).kind {
        UrlErrorKind::LimitExceeded { limit, .. } => Some(limit),
        _ => None,
    };
    // a concat 200 deep is refused by depth, before the parser's stack grows with it (T-4017)
    let deep = format!(
        "Buildings?$apply={}identity{}",
        "concat(identity,".repeat(200),
        ")".repeat(200)
    );
    assert_eq!(limit_of(&deep), Some(LimitKind::ExprDepth));
    // each transformation is a node: a chain past max_expr_nodes is refused
    let chain = format!("Buildings?$apply={}", ["identity"; 300].join("/"));
    assert_eq!(limit_of(&chain), Some(LimitKind::ExprNodes));
    // within the budget both still parse
    assert_eq!(apply_of(&["identity"; 40].join("/")).len(), 40);
    assert_eq!(
        apply_of("concat(concat(identity,identity),identity)").len(),
        1
    );
}
