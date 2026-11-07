// SPDX-License-Identifier: EUPL-1.2
//! `GeoJSON` geometries as ISO well-known binary, little-endian (OGC 06-103r4, as GeoParquet 1.1
//! encodes `WKB`): the seven geometry types, two or three coordinates per position.

use serde_json::Value;

/// One geometry as WKB, with the GeoParquet geometry type it is (`"Point"`, `"Polygon Z"`, …)
/// and its 2D bounding box `[xmin, ymin, xmax, ymax]`; `None` for anything that is no valid
/// `GeoJSON` geometry, which the file then holds as a null.
pub(crate) fn encode(geometry: &Value) -> Option<(Vec<u8>, String, Option<[f64; 4]>)> {
    let mut out = Vec::new();
    let mut bbox = None;
    let z = has_z(geometry)?;
    write(geometry, z, &mut out, &mut bbox)?;
    let name = geometry["type"].as_str()?;
    let name = if z {
        format!("{name} Z")
    } else {
        name.to_owned()
    };
    Some((out, name, bbox))
}

/// Whether the geometry's positions carry a height: the first position decides, and a position
/// of another length than the first is no valid geometry here.
fn has_z(geometry: &Value) -> Option<bool> {
    fn first(value: &Value) -> Option<&Vec<Value>> {
        let list = value.as_array()?;
        match list.first() {
            Some(Value::Array(_)) => list.iter().find_map(first),
            Some(_) => Some(list),
            None => None,
        }
    }
    let position = if geometry["type"] == "GeometryCollection" {
        geometry["geometries"]
            .as_array()?
            .iter()
            .find_map(|g| first(&g["coordinates"]))
    } else {
        first(&geometry["coordinates"])
    };
    // An empty geometry has no position; it is written in two dimensions.
    Some(position.is_some_and(|p| p.len() == 3))
}

fn code(name: &str) -> Option<u32> {
    Some(match name {
        "Point" => 1,
        "LineString" => 2,
        "Polygon" => 3,
        "MultiPoint" => 4,
        "MultiLineString" => 5,
        "MultiPolygon" => 6,
        "GeometryCollection" => 7,
        _ => return None,
    })
}

fn write(geometry: &Value, z: bool, out: &mut Vec<u8>, bbox: &mut Option<[f64; 4]>) -> Option<()> {
    let name = geometry["type"].as_str()?;
    let base = code(name)?;
    out.push(1);
    out.extend_from_slice(&(if z { base.checked_add(1000)? } else { base }).to_le_bytes());
    let coordinates = &geometry["coordinates"];
    match name {
        "Point" => position(coordinates, z, out, bbox),
        "LineString" => positions(coordinates, z, out, bbox),
        "Polygon" => rings(coordinates, z, out, bbox),
        "MultiPoint" | "MultiLineString" | "MultiPolygon" => {
            let parts = coordinates.as_array()?;
            count(parts.len(), out)?;
            let part = match name {
                "MultiPoint" => "Point",
                "MultiLineString" => "LineString",
                _ => "Polygon",
            };
            for coordinates in parts {
                let member = serde_json::json!({"type": part, "coordinates": coordinates});
                write(&member, z, out, bbox)?;
            }
            Some(())
        }
        _ => {
            let members = geometry["geometries"].as_array()?;
            count(members.len(), out)?;
            members.iter().try_for_each(|g| write(g, z, out, bbox))
        }
    }
}

fn count(n: usize, out: &mut Vec<u8>) -> Option<()> {
    out.extend_from_slice(&u32::try_from(n).ok()?.to_le_bytes());
    Some(())
}

fn rings(value: &Value, z: bool, out: &mut Vec<u8>, bbox: &mut Option<[f64; 4]>) -> Option<()> {
    let rings = value.as_array()?;
    count(rings.len(), out)?;
    rings
        .iter()
        .try_for_each(|ring| positions(ring, z, out, bbox))
}

fn positions(value: &Value, z: bool, out: &mut Vec<u8>, bbox: &mut Option<[f64; 4]>) -> Option<()> {
    let list = value.as_array()?;
    count(list.len(), out)?;
    list.iter().try_for_each(|p| position(p, z, out, bbox))
}

fn position(value: &Value, z: bool, out: &mut Vec<u8>, bbox: &mut Option<[f64; 4]>) -> Option<()> {
    let numbers: Vec<f64> = value
        .as_array()?
        .iter()
        .map(Value::as_f64)
        .collect::<Option<_>>()?;
    let (&x, &y) = (numbers.first()?, numbers.get(1)?);
    if numbers.len() != if z { 3 } else { 2 } {
        return None;
    }
    for n in &numbers {
        out.extend_from_slice(&n.to_le_bytes());
    }
    *bbox = Some(match *bbox {
        None => [x, y, x, y],
        Some([x0, y0, x1, y1]) => [x0.min(x), y0.min(y), x1.max(x), y1.max(y)],
    });
    Some(())
}

/// The coordinates of WKB as `GeoJSON` writes them, for the tests' read-back.
#[cfg(test)]
pub(crate) fn decode(bytes: &[u8]) -> Option<Value> {
    fn read(bytes: &[u8], at: &mut usize) -> Option<Value> {
        let take = |at: &mut usize, n: usize| -> Option<&[u8]> {
            let end = at.checked_add(n)?;
            let slice = bytes.get(*at..end)?;
            *at = end;
            Some(slice)
        };
        let u32_at = |at: &mut usize| -> Option<u32> {
            Some(u32::from_le_bytes(take(at, 4)?.try_into().ok()?))
        };
        (take(at, 1)? == [1]).then_some(())?;
        let code = u32_at(at)?;
        let (z, base) = (code > 1000, code % 1000);
        let dims = if z { 3 } else { 2 };
        let point = |at: &mut usize| -> Option<Value> {
            (0..dims)
                .map(|_| Some(f64::from_le_bytes(take(at, 8)?.try_into().ok()?)))
                .collect::<Option<Vec<f64>>>()
                .map(|p| serde_json::json!(p))
        };
        let line = |at: &mut usize| -> Option<Value> {
            let n = u32_at(at)?;
            (0..n)
                .map(|_| point(at))
                .collect::<Option<Vec<_>>>()
                .map(Value::Array)
        };
        let poly = |at: &mut usize| -> Option<Value> {
            let n = u32_at(at)?;
            (0..n)
                .map(|_| line(at))
                .collect::<Option<Vec<_>>>()
                .map(Value::Array)
        };
        match base {
            1 => point(at),
            2 => line(at),
            3 => poly(at),
            4..=7 => {
                let n = u32_at(at)?;
                (0..n)
                    .map(|_| read(bytes, at))
                    .collect::<Option<Vec<_>>>()
                    .map(Value::Array)
            }
            _ => None,
        }
    }
    let mut at = 0;
    let value = read(bytes, &mut at)?;
    (at == bytes.len()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_point_is_its_byte_order_type_and_coordinates() {
        let (bytes, name, bbox) =
            encode(&json!({"type": "Point", "coordinates": [19.15, 48.73]})).unwrap_or_default();
        assert_eq!(bytes.first(), Some(&1));
        assert_eq!(bytes.get(1..5), Some(&1u32.to_le_bytes()[..]));
        assert_eq!(bytes.len(), 21);
        assert_eq!(name, "Point");
        assert_eq!(bbox, Some([19.15, 48.73, 19.15, 48.73]));
        assert_eq!(decode(&bytes), Some(json!([19.15, 48.73])));
    }

    #[test]
    fn every_type_reads_back_to_its_coordinates() {
        for geometry in [
            json!({"type": "LineString", "coordinates": [[0.0, 0.0], [1.0, 2.0]]}),
            json!({"type": "Polygon", "coordinates": [[[0.0, 0.0], [4.0, 0.0], [4.0, 3.0], [0.0, 0.0]]]}),
            json!({"type": "MultiPoint", "coordinates": [[1.0, 1.0], [2.0, -2.0]]}),
            json!({"type": "MultiLineString", "coordinates": [[[0.0, 0.0], [1.0, 1.0]]]}),
            json!({"type": "MultiPolygon", "coordinates": [[[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 0.0]]]]}),
        ] {
            let (bytes, _, _) = encode(&geometry).unwrap_or_default();
            let back = decode(&bytes).unwrap_or_default();
            let expected = geometry.get("coordinates").cloned().unwrap_or_default();
            assert_eq!(back, expected, "{geometry}");
        }
    }

    #[test]
    fn a_height_is_iso_z_and_the_box_stays_two_dimensional() {
        let (bytes, name, bbox) = encode(
            &json!({"type": "LineString", "coordinates": [[0.0, 5.0, 100.0], [-1.0, 6.0, 120.0]]}),
        )
        .unwrap_or_default();
        assert_eq!(bytes.get(1..5), Some(&1002u32.to_le_bytes()[..]));
        assert_eq!(name, "LineString Z");
        assert_eq!(bbox, Some([-1.0, 5.0, 0.0, 6.0]));
        assert_eq!(
            decode(&bytes),
            Some(json!([[0.0, 5.0, 100.0], [-1.0, 6.0, 120.0]]))
        );
    }

    #[test]
    fn a_collection_holds_its_members() {
        let geometry = json!({"type": "GeometryCollection", "geometries": [
            {"type": "Point", "coordinates": [1.0, 2.0]},
            {"type": "LineString", "coordinates": [[3.0, 4.0], [5.0, 6.0]]}
        ]});
        let (bytes, name, bbox) = encode(&geometry).unwrap_or_default();
        assert_eq!(name, "GeometryCollection");
        assert_eq!(bbox, Some([1.0, 2.0, 5.0, 6.0]));
        assert_eq!(
            decode(&bytes),
            Some(json!([[1.0, 2.0], [[3.0, 4.0], [5.0, 6.0]]]))
        );
    }

    #[test]
    fn an_empty_geometry_is_two_dimensional() {
        let (bytes, name, bbox) =
            encode(&json!({"type": "LineString", "coordinates": []})).unwrap_or_default();
        assert_eq!(name, "LineString");
        assert_eq!(bbox, None);
        assert_eq!(decode(&bytes), Some(json!([])));
        assert_eq!(decode(&[1, 9, 0, 0, 0]), None, "no WKB type 9");
    }

    #[test]
    fn what_is_no_geometry_is_none() {
        for geometry in [
            Value::Null,
            json!({"type": "Circle", "coordinates": [0.0, 0.0]}),
            json!({"type": "Point", "coordinates": ["x", 0.0]}),
            json!({"type": "Point", "coordinates": [0.0]}),
            json!({"type": "LineString", "coordinates": [[0.0, 0.0], [1.0, 1.0, 1.0]]}),
            json!({"type": "Polygon", "coordinates": "nope"}),
        ] {
            assert!(encode(&geometry).is_none(), "{geometry}");
        }
    }
}
