// SPDX-License-Identifier: EUPL-1.2
//! `residual_equals_brute_force` (ADR-0028 Confirmation, the pure part): compiling an expression
//! and evaluating it on a random Entity answers what a direct reference evaluator, written
//! here without the crate's code, answers on the same Entity. The Entity's `GeoProperty` `g` is
//! a grid point, or a value that is no geometry, related to a literal point or box (T-3756).

use ngsild_residual::{compile, eval, Cmp, DateTime, Expr, Func, Lit, Path, Spatial, Truth};
use proptest::prelude::*;
use serde_json::{json, Map, Value};

const ATTRS: [&str; 3] = ["a", "b", "c"];
/// The slot of the `GeoProperty` `g` in a generated Entity, after [`ATTRS`].
const GEO: usize = ATTRS.len();

/// A leaf value an Attribute holds, or nothing.
#[derive(Debug, Clone, PartialEq)]
enum Leaf {
    Int(i64),
    Str(&'static str),
    Bool(bool),
    /// A `GeoJSON` Point, only ever in the `g` slot.
    Point(i32, i32),
}

/// A geometry literal: a point, or a box from its lower left to its upper right corner.
#[derive(Debug, Clone)]
enum Shape {
    Point(i32, i32),
    Box(i32, i32, i32, i32),
}

/// The reference form: what the generated expression means, evaluated with `Option<bool>`.
#[derive(Debug, Clone)]
enum Ref {
    Cmp(Cmp, usize, Leaf),
    AddCmp(Cmp, usize, i64, i64),
    IsNull(usize),
    In(usize, Vec<Leaf>),
    And(Vec<Ref>),
    Or(Vec<Ref>),
    Not(Box<Ref>),
    Geo(Spatial, Shape),
}

fn leaf() -> impl Strategy<Value = Leaf> {
    prop_oneof![
        prop_oneof![Just(i64::MIN), Just(i64::MAX), -3i64..=3].prop_map(Leaf::Int),
        prop_oneof![Just("x"), Just("y")].prop_map(Leaf::Str),
        any::<bool>().prop_map(Leaf::Bool),
    ]
}

fn op() -> impl Strategy<Value = Cmp> {
    prop_oneof![
        Just(Cmp::Eq),
        Just(Cmp::Ne),
        Just(Cmp::Lt),
        Just(Cmp::Le),
        Just(Cmp::Gt),
        Just(Cmp::Ge)
    ]
}

fn reference() -> impl Strategy<Value = Ref> {
    let attr = 0..ATTRS.len();
    let base = prop_oneof![
        (op(), attr.clone(), leaf()).prop_map(|(o, a, l)| Ref::Cmp(o, a, l)),
        (
            op(),
            attr.clone(),
            prop_oneof![Just(i64::MAX), -2i64..=2],
            -3i64..=3
        )
            .prop_map(|(o, a, k, v)| Ref::AddCmp(o, a, k, v)),
        attr.clone().prop_map(Ref::IsNull),
        (attr, prop::collection::vec(leaf(), 0..3)).prop_map(|(a, l)| Ref::In(a, l)),
        (spatial(), shape()).prop_map(|(r, s)| Ref::Geo(r, s)),
    ];
    base.prop_recursive(4, 32, 3, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..3).prop_map(Ref::And),
            prop::collection::vec(inner.clone(), 0..3).prop_map(Ref::Or),
            inner.prop_map(|r| Ref::Not(Box::new(r))),
        ]
    })
}

fn spatial() -> impl Strategy<Value = Spatial> {
    prop::sample::select(vec![
        Spatial::Intersects,
        Spatial::Within,
        Spatial::Contains,
        Spatial::Equals,
        Spatial::Disjoint,
        Spatial::Overlaps,
        Spatial::Touches,
        Spatial::Crosses,
    ])
}

fn shape() -> impl Strategy<Value = Shape> {
    prop_oneof![
        (0i32..=3, 0i32..=3).prop_map(|(x, y)| Shape::Point(x, y)),
        (span(), span()).prop_map(|((x0, x1), (y0, y1))| Shape::Box(x0, y0, x1, y1)),
    ]
}

/// A box side on the grid: a lower and a higher bound.
fn span() -> impl Strategy<Value = (i32, i32)> {
    prop::sample::select(vec![(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)])
}

fn entity() -> impl Strategy<Value = Vec<Option<Leaf>>> {
    let geo = prop::option::of(prop_oneof![
        (0i32..=3, 0i32..=3).prop_map(|(x, y)| Leaf::Point(x, y)),
        Just(Leaf::Str("POINT(1 1)")),
    ]);
    (
        prop::collection::vec(prop::option::of(leaf()), ATTRS.len()),
        geo,
    )
        .prop_map(|(mut e, g)| {
            e.push(g);
            e
        })
}

fn geojson_point(x: i32, y: i32) -> Value {
    json!({"type": "Point", "coordinates": [x, y]})
}

fn lit(l: &Leaf) -> Lit {
    match l {
        Leaf::Int(i) => Lit::Int(*i),
        Leaf::Str(s) => Lit::Str((*s).to_owned()),
        Leaf::Bool(b) => Lit::Bool(*b),
        Leaf::Point(x, y) => Lit::Geometry(geojson_point(*x, *y)),
    }
}

fn geometry(s: &Shape) -> Value {
    match s {
        Shape::Point(x, y) => geojson_point(*x, *y),
        Shape::Box(x0, y0, x1, y1) => json!({"type": "Polygon",
            "coordinates": [[[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]]]}),
    }
}

fn path(a: usize) -> Expr {
    Expr::Path(Path::value(ATTRS.get(a).copied().unwrap_or("a")))
}

fn expr(form: &Ref) -> Expr {
    match form {
        Ref::Cmp(op, attr, leaf) => {
            Expr::Cmp(*op, Box::new(path(*attr)), Box::new(Expr::Lit(lit(leaf))))
        }
        Ref::AddCmp(op, attr, addend, against) => Expr::Cmp(
            *op,
            Box::new(Expr::Call(
                Func::Add,
                vec![path(*attr), Expr::Lit(Lit::Int(*addend))],
            )),
            Box::new(Expr::Lit(Lit::Int(*against))),
        ),
        Ref::IsNull(attr) => Expr::IsNull(Box::new(path(*attr))),
        Ref::In(attr, list) => Expr::In(Box::new(path(*attr)), list.iter().map(lit).collect()),
        Ref::And(forms) => Expr::And(forms.iter().map(expr).collect()),
        Ref::Or(forms) => Expr::Or(forms.iter().map(expr).collect()),
        Ref::Not(inner) => Expr::Not(Box::new(expr(inner))),
        Ref::Geo(rel, s) => Expr::Spatial(
            *rel,
            Box::new(Expr::Path(Path::value("g"))),
            Box::new(Expr::Lit(Lit::Geometry(geometry(s)))),
        ),
    }
}

/// A point's relation to a point or a box, by hand: inside a box is within it, on its edge
/// touches it, outside is disjoint; a point never contains, overlaps or crosses a box.
fn relate(rel: Spatial, at: (i32, i32), s: &Shape) -> bool {
    let (meets, inside) = match *s {
        Shape::Point(px, py) => (at == (px, py), at == (px, py)),
        Shape::Box(x0, y0, x1, y1) => (
            (x0..=x1).contains(&at.0) && (y0..=y1).contains(&at.1),
            x0 < at.0 && at.0 < x1 && y0 < at.1 && at.1 < y1,
        ),
    };
    let point = matches!(s, Shape::Point(..));
    match rel {
        Spatial::Intersects => meets,
        Spatial::Within => inside,
        Spatial::Contains | Spatial::Equals => point && meets,
        Spatial::Disjoint => !meets,
        Spatial::Touches => meets && !inside,
        Spatial::Overlaps | Spatial::Crosses => false,
    }
}

fn holds(o: Cmp, ord: std::cmp::Ordering) -> bool {
    match o {
        Cmp::Eq => ord.is_eq(),
        Cmp::Ne => ord.is_ne(),
        Cmp::Lt => ord.is_lt(),
        Cmp::Le => ord.is_le(),
        Cmp::Gt => ord.is_gt(),
        Cmp::Ge => ord.is_ge(),
    }
}

/// SQL comparison: unknown for an absent side, a type mismatch, or an ordering of booleans.
fn compare(o: Cmp, x: Option<&Leaf>, y: &Leaf) -> Option<bool> {
    match (x?, y) {
        (Leaf::Int(x), Leaf::Int(y)) => Some(holds(o, x.cmp(y))),
        (Leaf::Str(x), Leaf::Str(y)) => Some(holds(o, x.cmp(y))),
        (Leaf::Bool(x), Leaf::Bool(y)) if matches!(o, Cmp::Eq | Cmp::Ne) => {
            Some(holds(o, x.cmp(y)))
        }
        _ => None,
    }
}

/// Kleene folding of the operands' answers: `decisive` wins, then unknown, else `!decisive`.
fn fold(answers: &[Option<bool>], decisive: bool) -> Option<bool> {
    if answers.contains(&Some(decisive)) {
        Some(decisive)
    } else if answers.contains(&None) {
        None
    } else {
        Some(!decisive)
    }
}

fn reference_eval(form: &Ref, entity: &[Option<Leaf>]) -> Option<bool> {
    let at = |attr: usize| entity.get(attr).and_then(Option::as_ref);
    match form {
        Ref::Cmp(op, attr, leaf) => compare(*op, at(*attr), leaf),
        Ref::AddCmp(op, attr, addend, against) => match at(*attr) {
            Some(Leaf::Int(value)) => value
                .checked_add(*addend)
                .map(|sum| holds(*op, sum.cmp(against))),
            _ => None,
        },
        Ref::IsNull(attr) => Some(at(*attr).is_none()),
        // An absent value is unknown even against an empty list; a present one is false there.
        Ref::In(attr, list) => at(*attr).and_then(|_| {
            let hits: Vec<Option<bool>> = list
                .iter()
                .map(|leaf| compare(Cmp::Eq, at(*attr), leaf))
                .collect();
            fold(&hits, true)
        }),
        Ref::And(forms) => fold(
            &forms
                .iter()
                .map(|f| reference_eval(f, entity))
                .collect::<Vec<_>>(),
            false,
        ),
        Ref::Or(forms) => fold(
            &forms
                .iter()
                .map(|f| reference_eval(f, entity))
                .collect::<Vec<_>>(),
            true,
        ),
        Ref::Not(inner) => reference_eval(inner, entity).map(|answer| !answer),
        Ref::Geo(rel, s) => match at(GEO) {
            Some(Leaf::Point(x, y)) => Some(relate(*rel, (*x, *y), s)),
            _ => None,
        },
    }
}

fn json_entity(e: &[Option<Leaf>]) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), json!("urn:ngsi-ld:T:1"));
    m.insert("type".into(), json!("T"));
    for (name, v) in ATTRS.iter().zip(e) {
        let value = match v {
            Some(Leaf::Int(i)) => json!(i),
            Some(Leaf::Str(s)) => json!(s),
            Some(Leaf::Bool(b)) => json!(b),
            Some(Leaf::Point(..)) | None => continue,
        };
        m.insert((*name).into(), json!({"type": "Property", "value": value}));
    }
    let g = match e.get(GEO) {
        Some(Some(Leaf::Point(x, y))) => geojson_point(*x, *y),
        Some(Some(Leaf::Str(s))) => json!(s),
        _ => return Value::Object(m),
    };
    m.insert("g".into(), json!({"type": "GeoProperty", "value": g}));
    Value::Object(m)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: if cfg!(miri) { 8 } else { 512 }, ..ProptestConfig::default() })]

    #[test]
    fn residual_equals_brute_force(r in reference(), e in entity()) {
        let program = compile(&expr(&r), DateTime::MIN);
        prop_assert!(program.is_ok(), "{r:?} did not compile");
        let got = program.ok().map(|p| eval(&p, &json_entity(&e)));
        let want = match reference_eval(&r, &e) {
            Some(true) => Truth::True,
            Some(false) => Truth::False,
            None => Truth::Unknown,
        };
        prop_assert_eq!(got, Some(Ok(want)), "{:?} on {:?}", r, e);
    }
}

/// The geometry cases are rare among random expressions, so every grid point against every
/// literal shape and relation, with the same reference.
#[test]
fn residual_equals_brute_force_on_every_grid_geometry() -> Result<(), String> {
    let mut shapes: Vec<Shape> = Vec::new();
    let spans = [(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)];
    for x in 0..=3 {
        for y in 0..=3 {
            shapes.push(Shape::Point(x, y));
        }
    }
    for (x0, x1) in spans {
        for (y0, y1) in spans {
            shapes.push(Shape::Box(x0, y0, x1, y1));
        }
    }
    let rels = [
        Spatial::Intersects,
        Spatial::Within,
        Spatial::Contains,
        Spatial::Equals,
        Spatial::Disjoint,
        Spatial::Overlaps,
        Spatial::Touches,
        Spatial::Crosses,
    ];
    for x in 0..=3 {
        for y in 0..=3 {
            let e = vec![None, None, None, Some(Leaf::Point(x, y))];
            for s in &shapes {
                for rel in rels {
                    let form = Ref::Geo(rel, s.clone());
                    let program =
                        compile(&expr(&form), DateTime::MIN).map_err(|e| e.to_string())?;
                    let got = eval(&program, &json_entity(&e)).map_err(|e| e.to_string())?;
                    let want = if reference_eval(&form, &e) == Some(true) {
                        Truth::True
                    } else {
                        Truth::False
                    };
                    if got != want {
                        return Err(format!(
                            "{rel:?} of ({x}, {y}) to {s:?}: {got:?}, want {want:?}"
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}
