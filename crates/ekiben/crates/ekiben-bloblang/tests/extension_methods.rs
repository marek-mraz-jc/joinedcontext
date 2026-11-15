use ekiben_bloblang::geo::{format_wkt, parse_wkt};
use ekiben_bloblang::ngsild::{check_geometry, EXTENSION_METHODS};
use ekiben_bloblang::{Environment, Value};
use serde_json::json;

fn run(mapping: &str, input: serde_json::Value) -> Result<Value, String> {
    Environment::new()
        .compile_mapping(mapping)
        .map_err(|e| e.to_string())?
        .exec_on_value(Value::from_json_value(input))
        .map_err(|e| e.to_string())
}

const BCN: &str = "POLYGON ((2.1734 41.3851, 2.18 41.3851, 2.18 41.39, 2.1734 41.3851))";

#[test]
fn barcelona_polygon_round_trips_through_parse_wkt_and_check_geometry() {
    let g = parse_wkt(BCN).expect("parses");
    check_geometry(&g, 0).expect("valid GeoJSON");
    assert_eq!(
        g.to_go_json(),
        r#"{"coordinates":[[[2.1734,41.3851],[2.18,41.3851],[2.18,41.39],[2.1734,41.3851]]],"type":"Polygon"}"#
    );
    assert_eq!(format_wkt(&g).as_deref(), Ok(BCN));
    assert_eq!(
        run(
            "root = this.wkt.parse_wkt().format_wkt()",
            json!({ "wkt": BCN })
        ),
        Ok(Value::String(BCN.to_string()))
    );
}

#[test]
fn wkt_dimensions_multipoints_and_collections() {
    let canon = |s: &str| format_wkt(&parse_wkt(s).expect(s)).expect(s);
    assert_eq!(
        parse_wkt("MULTIPOINT (1 2, 3 4)"),
        parse_wkt("MULTIPOINT ((1 2), (3 4))")
    );
    assert_eq!(canon("MULTIPOINT (1 2, 3 4)"), "MULTIPOINT ((1 2), (3 4))");
    assert_eq!(canon("point z(1 2 3)"), "POINT Z (1 2 3)");
    assert_eq!(canon("POINTZ (1 2 3)"), "POINT Z (1 2 3)");
    assert_eq!(canon("POINT (1 2 3)"), "POINT Z (1 2 3)");
    assert_eq!(
        canon("LINESTRING M (0 0 9, 1 1 9)"),
        "LINESTRING (0 0, 1 1)"
    );
    assert_eq!(canon("POINT ZM (1 2 3 4)"), "POINT Z (1 2 3)");
    assert_eq!(
        canon("GEOMETRYCOLLECTION (POINT (1 2), LINESTRING EMPTY)"),
        "GEOMETRYCOLLECTION (POINT (1 2), LINESTRING EMPTY)"
    );
    assert_eq!(
        canon("MULTIPOLYGON (((0 0, 1 0, 1 1, 0 0)), ((5 5, 6 5, 6 6, 5 5)))"),
        "MULTIPOLYGON (((0 0, 1 0, 1 1, 0 0)), ((5 5, 6 5, 6 6, 5 5)))"
    );
}

#[test]
fn bad_wkt_and_bad_geojson_are_errors() {
    for bad in [
        "",
        "POINT EMPTY",
        "POINT (nan 1)",
        "POINT (NaN 1)",
        "POINT (1e999 1)",
        "POINT (1 2) x",
        "POINT (1)",
        "POINT Z (1 2)",
        "LINESTRING (1 2, 3 4 5)",
        "CIRCLE (1 2)",
        "POLYGON ((1 2, 3 4)",
        "POINT (1 2é)",
        "é",
    ] {
        assert!(parse_wkt(bad).is_err(), "{bad:?}");
    }
    let deep = "GEOMETRYCOLLECTION (".repeat(100);
    assert!(parse_wkt(&deep).is_err());
    let point = |coords: Vec<Value>| {
        let mut m = std::collections::BTreeMap::new();
        m.insert("type".to_string(), Value::String("Point".into()));
        m.insert("coordinates".to_string(), Value::Array(coords.into()));
        Value::Object(m.into())
    };
    assert!(format_wkt(&point(vec![Value::Float(f64::NAN), Value::Int(1)])).is_err());
    assert!(format_wkt(&point(vec![Value::Int(1)])).is_err());
    assert!(format_wkt(&Value::String("POINT (1 2)".into())).is_err());
    assert!(run("root = this.wkt.parse_wkt()", json!({ "wkt": 5 })).is_err());
}

#[test]
fn fuzzed_wkt_never_panics_and_what_parses_round_trips() {
    const ALPHABET: &[u8] = b"POINTLINESTRGYCMZEPMULTI (),,..-+e0123456789  ";
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    for _ in 0..20_000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let len = (x % 48) as usize;
        let mut s = String::with_capacity(len);
        let mut y = x;
        for _ in 0..len {
            y ^= y << 13;
            y ^= y >> 7;
            y ^= y << 17;
            s.push(ALPHABET[(y % ALPHABET.len() as u64) as usize] as char);
        }
        for text in [
            s.clone(),
            format!("POINT ({s})"),
            format!("LINESTRING ({s})"),
        ] {
            if let Ok(g) = parse_wkt(&text) {
                let back = format_wkt(&g).unwrap_or_else(|e| panic!("{text:?}: {e}"));
                assert_eq!(parse_wkt(&back), Ok(g), "{text:?} -> {back:?}");
            }
        }
    }
}

#[test]
fn trigonometry_is_finite_or_an_error() {
    match run("root = this.y.atan2(this.x)", json!({"y": 1, "x": 1})) {
        Ok(Value::Float(f)) => assert!((f - std::f64::consts::FRAC_PI_4).abs() < 1e-15),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        run("root = this.v.sqrt()", json!({"v": 16})),
        Ok(Value::Float(4.0))
    );
    assert_eq!(
        run("root = this.v.atan()", json!({"v": 0})),
        Ok(Value::Float(0.0))
    );
    for (m, v) in [
        ("asin", 2.0),
        ("acos", -1.5),
        ("sqrt", -1.0),
        ("exp", 1000.0),
    ] {
        let err = run(&format!("root = this.v.{m}()"), json!({ "v": v })).unwrap_err();
        assert!(err.contains("not a finite number"), "{m}: {err}");
    }
    assert!(run("root = this.y.atan2()", json!({"y": 1}))
        .unwrap_err()
        .contains("missing parameter: x"));
    assert!(run("root = this.v.sqrt()", json!({"v": "a"})).is_err());
}

#[test]
fn geoip_methods_are_refused_and_extensions_are_known() {
    for name in ekiben_bloblang::methods::REFUSED_METHODS {
        let err = Environment::new()
            .compile_mapping(&format!("root = this.ip.{name}(path: \"db.mmdb\")"))
            .err()
            .unwrap_or_else(|| panic!("{name} compiled"))
            .to_string();
        assert!(err.contains("is refused"), "{name}: {err}");
    }
    let env = Environment::new();
    for name in EXTENSION_METHODS {
        assert!(env.has_method(name), "{name}");
    }
}

/// (EPSG, lon, lat, x, y): PROJ 9.8.1 (pyproj 3.8.0) on the same definitions as geo.rs, from WGS 84.
const PROJ_REFERENCE: &[(u32, f64, f64, f64, f64)] = &[
    (3879, 24.9525, 60.1695, 25497363.07008583, 6672958.394176747),
    (3067, 24.9525, 60.1695, 386392.1107469107, 6672049.50432971),
    (
        5514,
        14.421,
        50.0875,
        -742827.1496898808,
        -1043006.5559095312,
    ),
    (
        25829,
        -8.6291,
        41.1579,
        531118.7262371666,
        4556351.990189253,
    ),
    (
        25830,
        -3.7038,
        40.4168,
        440290.45805394615,
        4474257.381891726,
    ),
    (25831, 2.1734, 41.3851, 430887.564330584, 4581837.853239297),
    (25832, 9.99, 53.55, 565590.9406877231, 5933912.16541642),
    (25833, 13.405, 52.52, 391779.2592516289, 5820072.159087078),
    (
        25834,
        21.0122,
        52.2297,
        500833.24314743635,
        5786586.671118376,
    ),
    (3857, 2.1734, 41.3851, 241941.78129010077, 5069310.845092189),
    (2056, 7.4474, 46.948, 2600667.4752589185, 1199657.316392489),
    (
        27700,
        -0.1276,
        51.5072,
        530043.1949814068,
        180358.20862000342,
    ),
];

fn pair(v: &Value) -> (f64, f64) {
    match v {
        Value::Array(a) => (a[0].as_f64().expect("x"), a[1].as_f64().expect("y")),
        other => panic!("{other:?}"),
    }
}

#[test]
fn geo_transform_matches_proj_within_a_centimetre() {
    for &(code, lon, lat, x, y) in PROJ_REFERENCE {
        let crs = format!("EPSG:{code}");
        let mapping = "root = this.p.geo_transform(from: this.from, to: this.to)";
        let fwd = run(
            mapping,
            json!({"p": [lon, lat], "from": "EPSG:4326", "to": crs}),
        )
        .expect(&crs);
        let (fx, fy) = pair(&fwd);
        assert!((fx - x).hypot(fy - y) < 0.01, "{crs} forward: {fx} {fy}");
        let inv = run(
            mapping,
            json!({"p": [x, y], "from": crs, "to": "EPSG:4326"}),
        )
        .expect(&crs);
        let (ilon, ilat) = pair(&inv);
        let metres = ((ilon - lon) * lat.to_radians().cos()).hypot(ilat - lat) * 111_320.0;
        assert!(
            metres < 0.01,
            "{crs} inverse: {ilon} {ilat} is {metres} m off"
        );
    }
}

#[test]
fn geo_transform_reprojects_a_helsinki_wfs_geometry_for_check_geometry() {
    let wfs = json!({"type": "Polygon", "coordinates": [[
        [25497363.07, 6672958.39, 12.5], [25497463.07, 6672958.39, 12.5],
        [25497463.07, 6673058.39, 12.5], [25497363.07, 6672958.39, 12.5]
    ]]});
    let g = run(
        r#"root = this.geometry.geo_transform(from: "urn:ogc:def:crs:EPSG::3879", to: "WGS84")"#,
        json!({ "geometry": wfs }),
    )
    .expect("reprojects");
    check_geometry(&g, 0).expect("WGS 84 GeoJSON");
    let Value::Object(m) = &g else {
        panic!("{g:?}")
    };
    let Some(Value::Array(rings)) = m.get("coordinates") else {
        panic!("{g:?}")
    };
    let Value::Array(ring) = &rings[0] else {
        panic!("{g:?}")
    };
    let (lon, lat) = pair(&ring[0]);
    assert!(
        (lon - 24.9525).abs() < 1e-6 && (lat - 60.1695).abs() < 1e-6,
        "{lon} {lat}"
    );
    let Value::Array(first) = &ring[0] else {
        panic!()
    };
    assert_eq!(first[2].as_f64(), Some(12.5), "z is kept");
    let collection = json!({"type": "GeometryCollection", "geometries": [
        {"type": "Point", "coordinates": [25497363.07, 6672958.39]}
    ]});
    let c = run(
        r#"root = this.g.geo_transform("EPSG:3879", "EPSG:4326")"#,
        json!({ "g": collection }),
    )
    .expect("collection");
    check_geometry(&c, 0).expect("collection is WGS 84");
}

#[test]
fn geo_transform_errors_name_the_fault_and_nothing_panics() {
    let t = |p: serde_json::Value, from: &str, to: &str| {
        run(
            "root = this.p.geo_transform(from: this.f, to: this.t)",
            json!({"p": p, "f": from, "t": to}),
        )
    };
    let err = t(json!([1, 2]), "EPSG:31468", "EPSG:4326").unwrap_err();
    assert!(
        err.contains("parameter from") && err.contains("EPSG:3879"),
        "{err}"
    );
    assert!(t(json!([1, 2]), "EPSG:4326", "nadgrids=/etc/passwd")
        .unwrap_err()
        .contains("parameter to"));
    assert!(t(json!([10, 95]), "EPSG:4326", "EPSG:3857")
        .unwrap_err()
        .contains("latitude is out of range"));
    assert!(t(json!([1]), "EPSG:4326", "EPSG:3857").is_err());
    assert!(t(json!([1, "a"]), "EPSG:4326", "EPSG:3857").is_err());
    assert!(t(json!("POINT (1 2)"), "EPSG:4326", "EPSG:3857").is_err());
    assert!(t(json!({"type": "Point"}), "EPSG:4326", "EPSG:3857").is_err());
    assert!(run(
        "root = this.geo_transform(from: \"EPSG:4326\")",
        json!([1, 2])
    )
    .unwrap_err()
    .contains("missing parameter: to"));
    let codes: Vec<String> = PROJ_REFERENCE
        .iter()
        .map(|r| format!("EPSG:{}", r.0))
        .chain(["EPSG:4326".to_string()])
        .collect();
    let mut x: u64 = 0x2545_F491_4F6C_DD1D;
    for _ in 0..5_000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let a = f64::from_bits(x) % 1e9;
        let b = (x as i64 as f64) / 1e10;
        let (from, to) = (&codes[(x % 10) as usize], &codes[((x >> 8) % 10) as usize]);
        if let Ok(Value::Array(p)) = t(json!([a, b]), from, to) {
            assert!(
                p.iter().all(|n| n.as_f64().is_some_and(f64::is_finite)),
                "{from}->{to} {a} {b}"
            );
        }
    }
}
