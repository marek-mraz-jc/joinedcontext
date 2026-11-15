//! `unarchive` format `shapefile` against PROJ: the fixture layers in EPSG:3879 (Helsinki,
//! tests/fixtures/shapefile/make.py) come out as WGS 84 GeoJSON Features within 0.01 m of what
//! PROJ 9.8.1 (pyproj) gives for the same vertices, with their .dbf properties.

use ekiben_processors::shapefile::features;
use serde_json::{json, Value};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/shapefile");

/// Metres between two WGS 84 positions (equirectangular; exact enough at centimetres).
fn metres(a: &Value, b: &Value) -> f64 {
    let (lon_a, lat_a) = (a[0].as_f64().unwrap(), a[1].as_f64().unwrap());
    let (lon_b, lat_b) = (b[0].as_f64().unwrap(), b[1].as_f64().unwrap());
    let r = 6_371_008.8_f64;
    let dx = (lon_a - lon_b).to_radians() * lat_a.to_radians().cos() * r;
    let dy = (lat_a - lat_b).to_radians() * r;
    (dx * dx + dy * dy).sqrt()
}

fn reversed(ring: &Value) -> Vec<Value> {
    ring.as_array().unwrap().iter().rev().cloned().collect()
}

/// The geometry make.py's parts give: RFC 7946 winding (every ring reversed from the file's).
fn expected_geometry(kind: u64, parts: &Value) -> Value {
    let parts = parts.as_array().unwrap();
    match kind {
        0 => Value::Null,
        1 => json!({"type": "Point", "coordinates": parts[0][0]}),
        3 => json!({"type": "LineString", "coordinates": parts[0]}),
        5 => {
            // A square and the hole inside it (make.py's `hole` starts inside the square).
            let (outer, hole) = (&parts[0], &parts[1]);
            let inside = hole[0][1].as_f64() > outer[0][1].as_f64()
                && hole[0][0].as_f64() < outer[2][0].as_f64();
            if inside {
                json!({"type": "Polygon", "coordinates": [reversed(outer), reversed(hole)]})
            } else {
                json!({"type": "MultiPolygon", "coordinates": [[reversed(outer)], [reversed(hole)]]})
            }
        }
        _ => unreachable!("make.py writes no other shape"),
    }
}

fn assert_close(got: &Value, want: &Value, at: &str) {
    match (got, want) {
        (Value::Array(g), Value::Array(w)) if g.first().is_some_and(Value::is_number) => {
            let d = metres(got, want);
            assert!(d < 0.01, "{at}: {got} is {d} m from PROJ's {want}");
        }
        (Value::Array(g), Value::Array(w)) => {
            assert_eq!(g.len(), w.len(), "{at}: length");
            for (i, (g, w)) in g.iter().zip(w).enumerate() {
                assert_close(g, w, &format!("{at}[{i}]"));
            }
        }
        _ => assert_eq!(got, want, "{at}"),
    }
}

#[test]
fn helsinki_layers_come_out_as_wgs84_within_a_centimetre_of_proj() {
    let expected: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{FIXTURES}/expected.json")).unwrap(),
    )
    .unwrap();
    for (zip, want) in expected["layers"].as_object().unwrap() {
        let bytes = std::fs::read(format!("{FIXTURES}/{zip}")).unwrap();
        let got: Vec<Value> = features(&bytes, 100)
            .unwrap_or_else(|e| panic!("{zip}: {e}"))
            .iter()
            .map(|v| v.to_json_value())
            .collect();
        let want = want.as_array().unwrap();
        assert_eq!(
            got.len(),
            want.len(),
            "{zip}: the deleted record is skipped"
        );
        for (i, (g, w)) in got.iter().zip(want).enumerate() {
            assert_eq!(g["type"], "Feature");
            let geometry = expected_geometry(w["kind"].as_u64().unwrap(), &w["parts"]);
            assert_eq!(g["geometry"]["type"], geometry["type"], "{zip}[{i}]");
            assert_close(
                &g["geometry"]["coordinates"],
                &geometry["coordinates"],
                &format!("{zip}[{i}]"),
            );
            let mut props = w["properties"].clone();
            let pvm = props["PVM"].as_str().unwrap().to_string();
            props["PVM"] = if pvm.is_empty() {
                Value::Null
            } else {
                json!(format!("{}-{}-{}", &pvm[..4], &pvm[4..6], &pvm[6..]))
            };
            // Numbers compare by value: a Bloblang value writes 0.0 as 0, as Go's JSON does.
            let numbers = |v: &Value| -> Value {
                v.as_object()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| (k.clone(), v.as_f64().map_or(v.clone(), |n| json!(n))))
                    .collect()
            };
            assert_eq!(numbers(&g["properties"]), numbers(&props), "{zip}[{i}]");
        }
    }
}
