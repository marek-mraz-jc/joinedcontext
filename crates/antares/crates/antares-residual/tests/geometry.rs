// SPDX-License-Identifier: EUPL-1.2
//! Spatial relations of the residual (T-3756): each relation on the DE-9IM edge cases (a shared
//! boundary, a point on an edge, an empty intersection), the unknowns, the bounds, degenerate
//! rings, and the residual against the pushed `georel` matcher on the same geometries.

use std::collections::HashMap;

use antares_ql::geo::GeoQuery;
use ngsild_residual::{
    compile, eval, DateTime, Error, Expr, Lit, Path, Spatial, Truth, MAX_GEO_VERTICES,
};
use proptest::prelude::*;
use serde_json::{json, Value};

const ALL: [Spatial; 8] = [
    Spatial::Intersects,
    Spatial::Within,
    Spatial::Contains,
    Spatial::Equals,
    Spatial::Disjoint,
    Spatial::Overlaps,
    Spatial::Touches,
    Spatial::Crosses,
];

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Value {
    json!({"type": "Polygon", "coordinates": [[[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]]]})
}

fn point(x: f64, y: f64) -> Value {
    json!({"type": "Point", "coordinates": [x, y]})
}

fn line(points: &[[f64; 2]]) -> Value {
    json!({"type": "LineString", "coordinates": points})
}

fn located(geometry: &Value) -> Value {
    json!({"id": "urn:ngsi-ld:T:1", "type": "T",
           "location": {"type": "GeoProperty", "value": geometry}})
}

fn relation(rel: Spatial, literal: &Value) -> Expr {
    Expr::Spatial(
        rel,
        Box::new(Expr::Path(Path::value("location"))),
        Box::new(Expr::Lit(Lit::Geometry(literal.clone()))),
    )
}

fn truth(rel: Spatial, entity: &Value, literal: &Value) -> Result<Truth, Error> {
    eval(&compile(&relation(rel, literal), DateTime::MIN)?, entity)
}

/// The relations that hold, in [`ALL`] order, of the Entity's geometry to `literal`.
fn holding(geometry: &Value, literal: &Value) -> Result<Vec<Spatial>, Error> {
    let entity = located(geometry);
    let mut out = Vec::new();
    for rel in ALL {
        if truth(rel, &entity, literal)? == Truth::True {
            out.push(rel);
        }
    }
    Ok(out)
}

#[test]
fn each_relation_on_the_de9im_edge_cases() -> Result<(), Error> {
    use Spatial::{Contains, Crosses, Disjoint, Equals, Intersects, Overlaps, Touches, Within};
    let square = rect(0.0, 0.0, 2.0, 2.0);
    let cases: [(&str, Value, Vec<Spatial>); 10] = [
        ("point inside", point(1.0, 1.0), vec![Intersects, Within]),
        (
            "point on an edge",
            point(2.0, 1.0),
            vec![Intersects, Touches],
        ),
        (
            "point on a corner",
            point(0.0, 0.0),
            vec![Intersects, Touches],
        ),
        ("point outside", point(3.0, 3.0), vec![Disjoint]),
        (
            "square sharing an edge",
            rect(2.0, 0.0, 4.0, 2.0),
            vec![Intersects, Touches],
        ),
        (
            "square overlapping",
            rect(1.0, 1.0, 3.0, 3.0),
            vec![Intersects, Overlaps],
        ),
        (
            "square holding it",
            rect(-1.0, -1.0, 3.0, 3.0),
            vec![Intersects, Contains],
        ),
        (
            "the same square, ring started elsewhere",
            json!({"type": "Polygon", "coordinates": [[[2, 2], [0, 2], [0, 0], [2, 0], [2, 2]]]}),
            vec![Intersects, Within, Contains, Equals],
        ),
        (
            "line across it",
            line(&[[-1.0, 1.0], [3.0, 1.0]]),
            vec![Intersects, Crosses],
        ),
        (
            "line far away",
            line(&[[5.0, 5.0], [6.0, 6.0]]),
            vec![Disjoint],
        ),
    ];
    for (name, geometry, expected) in cases {
        assert_eq!(holding(&geometry, &square)?, expected, "{name}");
    }
    Ok(())
}

#[test]
fn what_is_no_geometry_is_unknown_and_stays_unknown_under_not() -> Result<(), Error> {
    let square = rect(0.0, 0.0, 2.0, 2.0);
    let entities = [
        json!({"id": "urn:ngsi-ld:T:1", "type": "T"}),
        located(&json!({"type": "Point", "coordinates": "1,1"})),
        located(&json!({"type": "Circle", "coordinates": [1, 1]})),
        located(&json!({"type": "Point", "coordinates": [1, 95]})),
        located(&json!("POINT(1 1)")),
        json!({"id": "urn:ngsi-ld:T:1", "type": "T",
               "location": {"type": "Property", "value": 7}}),
    ];
    for entity in &entities {
        for rel in ALL {
            assert_eq!(
                truth(rel, entity, &square)?,
                Truth::Unknown,
                "{rel:?} {entity}"
            );
            let negated = Expr::Not(Box::new(relation(rel, &square)));
            assert_eq!(
                eval(&compile(&negated, DateTime::MIN)?, entity)?,
                Truth::Unknown
            );
        }
    }
    Ok(())
}

#[test]
fn a_literal_is_checked_once_at_compile() {
    // the bound of the NGSI-LD geoQ geometry, so a literal the pushed georel takes compiles here
    assert_eq!(MAX_GEO_VERTICES, antares_ql::geo::MAX_GEO_VERTICES);
    let compiled =
        |literal: Value| compile(&relation(Spatial::Intersects, &literal), DateTime::MIN);
    let ring: Vec<[f64; 2]> = (0..=MAX_GEO_VERTICES)
        .map(|i| [f64::from(u32::try_from(i % 90).unwrap_or(0)), 0.0])
        .collect();
    assert_eq!(
        compiled(json!({"type": "LineString", "coordinates": ring})).err(),
        Some(Error::TooComplex)
    );
    for bad in [
        json!({"type": "Point"}),
        json!({"type": "Point", "coordinates": [181, 0]}),
        json!({"type": "GeometryCollection", "coordinates": []}),
        json!({"type": "Polygon", "coordinates": [[[0, 0], [1, 0], [0, 0]]]}),
        json!(42),
    ] {
        assert!(
            matches!(compiled(bad.clone()), Err(Error::Invalid(_))),
            "{bad}"
        );
    }
}

#[test]
fn a_value_past_the_vertex_bound_is_unknown() -> Result<(), Error> {
    let long: Vec<[f64; 2]> = (0..=MAX_GEO_VERTICES)
        .map(|i| [1.0, f64::from(u32::try_from(i % 2).unwrap_or(0))])
        .collect();
    let entity = located(&json!({"type": "LineString", "coordinates": long}));
    assert_eq!(
        truth(Spatial::Intersects, &entity, &rect(0.0, 0.0, 2.0, 2.0))?,
        Truth::Unknown
    );
    Ok(())
}

#[test]
fn degenerate_rings_relate_without_a_panic() -> Result<(), Error> {
    let degenerate = [
        json!({"type": "Polygon", "coordinates": [[[1, 1], [1, 1], [1, 1], [1, 1]]]}),
        json!({"type": "Polygon", "coordinates": [[[0, 0], [1, 1], [2, 2], [0, 0]]]}),
        json!({"type": "Polygon", "coordinates": [[[0, 0], [2, 2], [2, 0], [0, 2], [0, 0]]]}),
        json!({"type": "MultiPolygon", "coordinates": []}),
        json!({"type": "LineString", "coordinates": [[1, 1], [1, 1]]}),
    ];
    for g in &degenerate {
        for other in &degenerate {
            for rel in ALL {
                // literal or value: both sides go through the same parse and relate
                truth(rel, &located(g), other)?;
            }
        }
        for rel in ALL {
            truth(rel, &located(g), &rect(0.0, 0.0, 2.0, 2.0))?;
        }
    }
    Ok(())
}

#[test]
fn two_values_of_one_entity_relate() -> Result<(), Error> {
    let entity = json!({"id": "urn:ngsi-ld:T:1", "type": "T",
        "location": {"type": "GeoProperty", "value": point(1.0, 1.0)},
        "area": {"type": "GeoProperty", "value": rect(0.0, 0.0, 2.0, 2.0)}});
    let e = Expr::Spatial(
        Spatial::Within,
        Box::new(Expr::Path(Path::value("location"))),
        Box::new(Expr::Path(Path::value("area"))),
    );
    assert_eq!(eval(&compile(&e, DateTime::MIN)?, &entity)?, Truth::True);
    Ok(())
}

/// A small geometry on a grid where every DE-9IM case shows up: points, segments and boxes.
fn shape() -> impl Strategy<Value = Value> {
    let c = -1i32..=3;
    prop_oneof![
        (c.clone(), c.clone()).prop_map(|(x, y)| point(f64::from(x), f64::from(y))),
        (c.clone(), c.clone(), c.clone(), c.clone()).prop_filter_map(
            "a segment",
            |(a, b, x, y)| {
                ((a, b) != (x, y))
                    .then(|| line(&[[f64::from(a), f64::from(b)], [f64::from(x), f64::from(y)]]))
            }
        ),
        (c.clone(), c, prop::sample::select(vec![1.0, 2.0, 3.0])).prop_map(|(x, y, side)| {
            let (x, y) = (f64::from(x), f64::from(y));
            rect(x, y, x + side, y + side)
        }),
    ]
}

fn pushed(rel: &str, query: &Value) -> Option<GeoQuery> {
    let mut params = HashMap::new();
    params.insert("georel".to_owned(), rel.to_owned());
    params.insert(
        "geometry".to_owned(),
        query.get("type")?.as_str()?.to_owned(),
    );
    params.insert(
        "coordinates".to_owned(),
        query.get("coordinates")?.to_string(),
    );
    GeoQuery::from_params(&params).ok().flatten()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: if cfg!(miri) { 4 } else { 512 }, ..ProptestConfig::default() })]

    /// The residual relation keeps exactly the Entities the pushed `georel` of the same name
    /// keeps (the in-memory matcher the stores without a spatial index run).
    #[test]
    fn residual_relation_equals_pushed_georel(target in shape(), query in shape()) {
        let entity = located(&target);
        for (rel, georel) in [
            (Spatial::Intersects, "intersects"),
            (Spatial::Within, "within"),
            (Spatial::Contains, "contains"),
            (Spatial::Equals, "equals"),
            (Spatial::Disjoint, "disjoint"),
            (Spatial::Overlaps, "overlaps"),
        ] {
            let q = pushed(georel, &query).ok_or_else(|| TestCaseError::fail("a georel query"))?;
            let residual = truth(rel, &entity, &query).map_err(|e| TestCaseError::fail(e.to_string()))?;
            prop_assert_eq!(residual == Truth::True, q.matches_geometry(&target), "{} {} {}", georel, target, query);
        }
    }
}

#[test]
fn multi_geometries_relate_by_their_members() -> Result<(), Error> {
    let square = rect(0.0, 0.0, 2.0, 2.0);
    let some_in = located(&json!({"type": "MultiPoint", "coordinates": [[1, 1], [5, 5]]}));
    let lines = located(&json!({"type": "MultiLineString",
        "coordinates": [[[5, 5], [6, 6]], [[2, 0], [2, 2]]]}));
    assert_eq!(truth(Spatial::Intersects, &some_in, &square)?, Truth::True);
    assert_eq!(truth(Spatial::Within, &some_in, &square)?, Truth::False);
    assert_eq!(truth(Spatial::Touches, &lines, &square)?, Truth::True);
    assert_eq!(truth(Spatial::Overlaps, &lines, &square)?, Truth::False);
    Ok(())
}

#[test]
fn a_geometry_literal_compares_with_nothing() -> Result<(), Error> {
    let p = point(1.0, 1.0);
    let e = Expr::Cmp(
        ngsild_residual::Cmp::Eq,
        Box::new(Expr::Path(Path::value("location"))),
        Box::new(Expr::Lit(Lit::Geometry(p.clone()))),
    );
    assert_eq!(
        eval(&compile(&e, DateTime::MIN)?, &located(&p))?,
        Truth::Unknown
    );
    Ok(())
}
