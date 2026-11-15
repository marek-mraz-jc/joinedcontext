use ekiben_bloblang::gml::parse_gml;
use ekiben_bloblang::ngsild::{check_geometry, EXTENSION_METHODS};
use ekiben_bloblang::{Environment, Value};
use serde_json::json;

const FMI: &str =
    include_str!("../../../../../deploy/tests/fixtures/helsinki/fmi-airquality-hourly.xml");
const PARKING: &str = include_str!(
    "../../../../../deploy/tests/fixtures/helsinki/kartta/wfs11-Pysakointipaikat_alue-EPSG3879.xml"
);
const DISTRICT: &str = include_str!(
    "../../../../../deploy/tests/fixtures/helsinki/kartta/wfs11-Kaupunginosajako-EPSG3879.xml"
);

fn run(mapping: &str, input: serde_json::Value) -> Result<Value, String> {
    Environment::new()
        .compile_mapping(mapping)
        .map_err(|e| e.to_string())?
        .exec_on_value(Value::from_json_value(input))
        .map_err(|e| e.to_string())
}

fn text(s: &str) -> Value {
    Value::String(s.to_string())
}

/// The first `<gml:{name} ...>...</gml:{name}>` of a document.
fn fragment<'a>(doc: &'a str, name: &str) -> &'a str {
    let start = doc.find(&format!("<gml:{name} ")).expect(name);
    let end = format!("</gml:{name}>");
    let stop = doc[start..].find(&end).expect(name) + start + end.len();
    &doc[start..stop]
}

fn coords(g: &Value) -> &Value {
    match g {
        Value::Object(m) => m.get("coordinates").expect("coordinates"),
        other => panic!("{other:?}"),
    }
}

fn kind(g: &Value) -> String {
    match g {
        Value::Object(m) => m.get("type").expect("type").to_string(),
        other => panic!("{other:?}"),
    }
}

fn items(v: &Value) -> &[Value] {
    match v {
        Value::Array(a) => a.as_slice(),
        other => panic!("{other:?}"),
    }
}

fn xy(p: &Value) -> (f64, f64) {
    let a = items(p);
    (a[0].as_f64().expect("x"), a[1].as_f64().expect("y"))
}

fn in_helsinki((lon, lat): (f64, f64)) -> bool {
    (24.8..25.3).contains(&lon) && (60.1..60.3).contains(&lat)
}

#[test]
fn parse_gml_is_an_extension_method() {
    assert!(EXTENSION_METHODS.contains(&"parse_gml"));
}

#[test]
fn fmi_wfs2_point_in_etrs89_lat_lon_becomes_lon_lat() {
    let point = fragment(FMI, "Point");
    let g = run(
        r#"root = this.g.parse_gml(to: "EPSG:4326")"#,
        json!({ "g": point }),
    )
    .expect("FMI point");
    check_geometry(&g, 0).expect("WGS 84 GeoJSON");
    let (lon, lat) = xy(coords(&g));
    assert!(
        (lon - 24.95060).abs() < 1e-6 && (lat - 60.18739).abs() < 1e-6,
        "{lon} {lat}"
    );
    // Without `to` the axes are swapped all the same.
    assert_eq!(
        xy(coords(&parse_gml(&text(point), None, None).unwrap())),
        (24.9506, 60.18739)
    );
}

#[test]
fn kartta_wfs11_multisurface_in_gk25_reprojects_from_text_and_from_parse_xml() {
    let from_text = parse_gml(
        &text(fragment(PARKING, "MultiSurface")),
        None,
        Some("EPSG:4326"),
    )
    .expect("text");
    assert_eq!(kind(&from_text), "MultiPolygon");
    check_geometry(&from_text, 0).expect("WGS 84 GeoJSON");
    let ring = &items(&items(&items(coords(&from_text))[0])[0]);
    assert_eq!(ring.len(), 5);
    assert!(ring.iter().all(|p| in_helsinki(xy(p))), "{ring:?}");

    // A mapping that parses the whole WFS answer holds the object, not the text.
    let from_object = run(
        r#"root = this.doc.parse_xml().FeatureCollection.featureMembers.Pysakointipaikat_alue.index(0).geom.parse_gml(to: "EPSG:4326")"#,
        json!({ "doc": PARKING }),
    )
    .expect("parse_xml object");
    assert_eq!(from_object, from_text);
}

#[test]
fn kartta_ring_of_arc_strings_is_densified_into_a_closed_polygon() {
    let polygon = fragment(DISTRICT, "Polygon");
    let g = parse_gml(&text(polygon), None, Some("WGS84")).expect("district");
    assert_eq!(kind(&g), "Polygon");
    check_geometry(&g, 0).expect("WGS 84 GeoJSON");
    let ring = items(&items(coords(&g))[0]);
    let in_gml = polygon.matches(' ').count();
    assert!(ring.len() > 20, "arcs add chords: {}", ring.len());
    assert!(ring.len() < in_gml * 40, "{}", ring.len());
    assert!(ring.iter().all(|p| in_helsinki(xy(p))));
    assert_eq!(ring.first(), ring.last());
    let object = run(
        r#"root = this.doc.parse_xml().FeatureCollection.featureMembers.Kaupunginosajako.geom.parse_gml(to: "WGS84")"#,
        json!({ "doc": DISTRICT }),
    )
    .expect("object");
    assert_eq!(object, g);
}

#[test]
fn an_arc_through_three_points_stays_on_its_circle() {
    let gml = r#"<gml:Curve srsName="EPSG:3879"><gml:segments><gml:ArcString><gml:posList>10 0 0 10 -10 0</gml:posList></gml:ArcString><gml:LineStringSegment><gml:posList>-10 0 -10 -5</gml:posList></gml:LineStringSegment></gml:segments></gml:Curve>"#;
    let g = parse_gml(&text(gml), None, None).expect("curve");
    let ps = items(coords(&g));
    assert_eq!(xy(&ps[0]), (10.0, 0.0));
    assert_eq!(xy(ps.last().unwrap()), (-10.0, -5.0));
    let arc = &ps[..ps.len() - 1];
    assert_eq!(arc.len(), 37, "180 degrees in 5 degree chords");
    for p in arc {
        let (x, y) = xy(p);
        assert!((x.hypot(y) - 10.0).abs() < 1e-9 && y >= -1e-9, "{x} {y}");
    }
    // A clockwise arc sweeps the other way, below the x axis.
    let g = parse_gml(&text(r#"<gml:Curve><gml:segments><gml:Arc><gml:posList>10 0 0 -10 -10 0</gml:posList></gml:Arc></gml:segments></gml:Curve>"#), None, None).unwrap();
    assert!(items(coords(&g)).iter().all(|p| xy(p).1 <= 1e-9));
    // Three collinear positions are a straight line.
    let g = parse_gml(&text(r#"<gml:Curve><gml:segments><gml:ArcString><gml:posList>0 0 1 1 2 2</gml:posList></gml:ArcString></gml:segments></gml:Curve>"#), None, None).unwrap();
    assert_eq!(items(coords(&g)).len(), 3);
}

#[test]
fn gml2_coordinates_and_multi_geometries() {
    let poly = r#"<gml:Polygon srsName="EPSG:4326"><gml:outerBoundaryIs><gml:LinearRing><gml:coordinates decimal="," cs=";" ts=" ">24,9;60,1 25,0;60,1 25,0;60,2 24,9;60,1</gml:coordinates></gml:LinearRing></gml:outerBoundaryIs><gml:innerBoundaryIs><gml:LinearRing><gml:coordinates>24.95,60.12 24.96,60.12 24.96,60.13 24.95,60.12</gml:coordinates></gml:LinearRing></gml:innerBoundaryIs></gml:Polygon>"#;
    let g = parse_gml(&text(poly), None, None).expect("GML 2 polygon");
    check_geometry(&g, 0).expect("valid");
    let rings = items(coords(&g));
    assert_eq!(rings.len(), 2);
    assert_eq!(
        xy(&items(&rings[0])[0]),
        (24.9, 60.1),
        "short EPSG form is x/y"
    );

    let multi = r#"<gml:MultiGeometry srsName="urn:ogc:def:crs:EPSG::4326">
        <gml:name>both</gml:name>
        <gml:geometryMember><gml:MultiPoint><gml:pointMember><gml:Point><gml:pos>60.1 24.9</gml:pos></gml:Point></gml:pointMember><gml:pointMembers><gml:Point><gml:coord><gml:X>60.2</gml:X><gml:Y>25.0</gml:Y></gml:coord></gml:Point></gml:pointMembers></gml:MultiPoint></gml:geometryMember>
        <gml:geometryMember><gml:MultiCurve><gml:curveMember><gml:LineString><gml:pos>60.1 24.9</gml:pos><gml:pos>60.2 25.0</gml:pos></gml:LineString></gml:curveMember></gml:MultiCurve></gml:geometryMember>
        <gml:geometryMember><gml:LineString srsDimension="3"><gml:posList>60.1 24.9 5 60.2 25.0 6</gml:posList></gml:LineString></gml:geometryMember>
    </gml:MultiGeometry>"#;
    let g = parse_gml(&text(multi), None, None).expect("collection");
    check_geometry(&g, 0).expect("valid");
    let Value::Object(m) = &g else { panic!() };
    let gs = items(m.get("geometries").unwrap());
    assert_eq!(
        gs.iter().map(kind).collect::<Vec<_>>(),
        ["MultiPoint", "MultiLineString", "LineString"]
    );
    let mp = items(coords(&gs[0]));
    assert_eq!(
        (xy(&mp[0]), xy(&mp[1])),
        ((24.9, 60.1), (25.0, 60.2)),
        "URN 4326 is lat/lon"
    );
    assert_eq!(
        items(&items(coords(&gs[2]))[1])[2].as_f64(),
        Some(6.0),
        "z kept"
    );
}

#[test]
fn from_names_the_crs_of_a_gml_without_srs_name() {
    let p = text("<gml:Point><gml:pos>6672958.39 25497363.07</gml:pos></gml:Point>");
    let err = parse_gml(&p, None, Some("EPSG:4326")).unwrap_err();
    assert!(err.contains("parameter from"), "{err}");
    let g = parse_gml(&p, Some("urn:ogc:def:crs:EPSG::3879"), Some("EPSG:4326")).unwrap();
    let (lon, lat) = xy(coords(&g));
    assert!(
        (lon - 24.9525).abs() < 1e-6 && (lat - 60.1695).abs() < 1e-6,
        "{lon} {lat}"
    );
    let positional = run(
        r#"root = this.p.parse_gml("urn:ogc:def:crs:EPSG::3879", "EPSG:4326")"#,
        json!({ "p": "<gml:Point><gml:pos>6672958.39 25497363.07</gml:pos></gml:Point>" }),
    )
    .unwrap();
    assert_eq!(positional, g);
}

#[test]
fn malformed_gml_is_an_error_naming_the_fault() {
    let cases: &[(&str, &str)] = &[
        (r#"<!DOCTYPE x [<!ENTITY e "1">]><gml:Point><gml:pos>&e; 2</gml:pos></gml:Point>"#, "forbidden"),
        ("<gml:Point><gml:pos>1 &bogus;</gml:pos></gml:Point>", "unknown entity"),
        ("<gml:Point><gml:pos>NaN 2</gml:pos></gml:Point>", "finite"),
        ("<gml:Point><gml:pos>inf 2</gml:pos></gml:Point>", "finite"),
        ("<gml:Point><gml:pos>1</gml:pos></gml:Point>", "position of 1"),
        ("<gml:LineString><gml:posList>1 2 3</gml:posList></gml:LineString>", "multiple"),
        (r#"<gml:LineString srsDimension="4"><gml:posList>1 2 3 4</gml:posList></gml:LineString>"#, "srsDimension"),
        ("<gml:Point><gml:pos>1 2</gml:pos>", "failed to parse GML"),
        ("<gml:Point><gml:pos>1 2</gml:pos></gml:Point><gml:Point/>", "after the geometry"),
        ("<gml:Polygon><gml:exterior><gml:LinearRing><gml:posList>0 0 1 0 1 1 0 1</gml:posList></gml:LinearRing></gml:exterior></gml:Polygon>", "last equal to the first"),
        ("<gml:Polygon></gml:Polygon>", "needs an exterior"),
        ("<gml:Point><gml:foo>1 2</gml:foo></gml:Point>", "unexpected gml:foo"),
        ("<gml:Envelope><gml:lowerCorner>1 2</gml:lowerCorner></gml:Envelope>", "not a GML geometry"),
        (r#"<gml:Point srsName="urn:ogc:def:crs:EPSG::31468"><gml:pos>1 2</gml:pos></gml:Point>"#, "axis order"),
        (r#"<gml:Point srsName="urn:ogc:def:crs:OGC::foo"><gml:pos>1 2</gml:pos></gml:Point>"#, "no EPSG"),
        (r#"<gml:MultiPoint srsName="EPSG:4326"><gml:pointMember><gml:Point srsName="EPSG:3879"><gml:pos>1 2</gml:pos></gml:Point></gml:pointMember></gml:MultiPoint>"#, "srsName"),
        ("<gml:LineString><gml:posList>0 0 1 1</gml:posList><gml:posList>", "failed to parse GML"),
        ("<gml:Curve><gml:segments><gml:ArcString><gml:posList>0 0 1 1</gml:posList></gml:ArcString></gml:segments></gml:Curve>", "2n+1"),
        ("", "empty document"),
    ];
    for (gml, want) in cases {
        let err = parse_gml(&text(gml), None, None).expect_err(gml);
        assert!(err.contains(want), "{gml}: {err}");
    }
    assert!(parse_gml(&Value::Int(5), None, None)
        .unwrap_err()
        .contains("got"));
    assert!(parse_gml(&Value::from_json_value(json!({"a": 1, "b": 2})), None, None).is_err());
    // parse_xml groups mixed siblings by name, so a mixed Curve's order is gone.
    let mixed = run(
        "root = this.g.parse_xml().parse_gml()",
        json!({"g": "<gml:Curve><gml:segments><gml:LineStringSegment><gml:posList>0 0 1 1</gml:posList></gml:LineStringSegment><gml:ArcString><gml:posList>1 1 2 0 3 1</gml:posList></gml:ArcString></gml:segments></gml:Curve>"}),
    )
    .unwrap_err();
    assert!(mixed.contains("pass the GML text"), "{mixed}");
}

#[test]
fn deep_nesting_is_refused_without_a_stack_overflow() {
    let deep = |n: usize| {
        format!(
            "{}<gml:Point><gml:pos>1 2</gml:pos></gml:Point>{}",
            "<gml:MultiGeometry><gml:geometryMember>".repeat(n),
            "</gml:geometryMember></gml:MultiGeometry>".repeat(n)
        )
    };
    assert!(parse_gml(&text(&deep(8)), None, None).is_ok());
    assert!(parse_gml(&text(&deep(9)), None, None)
        .unwrap_err()
        .contains("deeper than 8"));
    let elements = format!("{}{}", "<a>".repeat(100_000), "</a>".repeat(100_000));
    assert!(parse_gml(&text(&elements), None, None)
        .unwrap_err()
        .contains("nest deeper than 32"));
}

#[test]
fn fuzzed_gml_never_panics() {
    let seeds = [
        fragment(FMI, "Point").to_string(),
        fragment(PARKING, "MultiSurface").to_string(),
        fragment(DISTRICT, "Polygon").to_string(),
    ];
    let alphabet = b"<>/=\" :.-+eE0123456789gml";
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for i in 0..3_000 {
        let mut b = seeds[i % seeds.len()].clone().into_bytes();
        for _ in 0..(next() % 6 + 1) {
            let at = (next() as usize) % b.len();
            match next() % 3 {
                0 => b[at] = alphabet[(next() as usize) % alphabet.len()],
                1 => {
                    b.remove(at);
                }
                _ => b.insert(at, alphabet[(next() as usize) % alphabet.len()]),
            }
        }
        let s = String::from_utf8_lossy(&b).into_owned();
        if let Ok(g) = parse_gml(&text(&s), None, None) {
            assert!(matches!(g, Value::Object(_)), "{s}");
        }
        let _ = parse_gml(&text(&s), Some("EPSG:3879"), Some("EPSG:4326"));
    }
}
