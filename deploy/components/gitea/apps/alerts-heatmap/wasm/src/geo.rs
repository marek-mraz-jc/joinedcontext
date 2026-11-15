//! Metres around Helsinki and back, a point that stands for any GeoJSON geometry, and the pointy-top
//! hexagons the map bins alerts into. An equirectangular projection about the city centre is off by
//! well under one per cent over the capital region, which a heat map does not see.

use serde_json::Value;

const EARTH_RADIUS_M: f64 = 6_371_008.8;
/// Helsinki's centre, the origin of the local metres.
pub const ORIGIN: (f64, f64) = (24.9384, 60.1699);

fn metres_per_degree() -> (f64, f64) {
    let per_degree = EARTH_RADIUS_M * std::f64::consts::PI / 180.0;
    (per_degree * ORIGIN.1.to_radians().cos(), per_degree)
}

/// Longitude and latitude to metres east and north of the centre.
pub fn to_metres(lon: f64, lat: f64) -> (f64, f64) {
    let (mx, my) = metres_per_degree();
    ((lon - ORIGIN.0) * mx, (lat - ORIGIN.1) * my)
}

/// Metres east and north of the centre back to longitude and latitude.
pub fn to_lon_lat(x: f64, y: f64) -> (f64, f64) {
    let (mx, my) = metres_per_degree();
    (ORIGIN.0 + x / mx, ORIGIN.1 + y / my)
}

fn push_positions(value: &Value, out: &mut Vec<(f64, f64)>) {
    let Some(items) = value.as_array() else {
        return;
    };
    if let [Value::Number(lon), Value::Number(lat), ..] = items.as_slice() {
        if let (Some(lon), Some(lat)) = (lon.as_f64(), lat.as_f64()) {
            if lon.is_finite()
                && lat.is_finite()
                && (-180.0..=180.0).contains(&lon)
                && (-90.0..=90.0).contains(&lat)
            {
                out.push((lon, lat));
            }
        }
        return;
    }
    for item in items {
        push_positions(item, out);
    }
}

/// The point that stands for a geometry: the mean of its positions (a road work along a street
/// is drawn at the middle of the street). `None` for a missing or empty geometry.
pub fn representative_point(geometry: &Value) -> Option<(f64, f64)> {
    let mut positions = Vec::new();
    match geometry.get("type").and_then(Value::as_str) {
        Some("GeometryCollection") => {
            for part in geometry
                .get("geometries")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                push_positions(
                    part.get("coordinates").unwrap_or(&Value::Null),
                    &mut positions,
                );
            }
        }
        Some(_) => push_positions(
            geometry.get("coordinates").unwrap_or(&Value::Null),
            &mut positions,
        ),
        None => return None,
    }
    if positions.is_empty() {
        return None;
    }
    let n = positions.len() as f64;
    let (lon, lat) = positions
        .iter()
        .fold((0.0, 0.0), |(a, b), (lon, lat)| (a + lon, b + lat));
    Some((lon / n, lat / n))
}

/// The axial coordinates (q, r) of the pointy-top hexagon of circumradius `size` holding a point.
pub fn hex_of(x: f64, y: f64, size: f64) -> (i64, i64) {
    let q = (3f64.sqrt() / 3.0 * x - y / 3.0) / size;
    let r = (2.0 / 3.0 * y) / size;
    // Cube rounding: the nearest hexagon centre, ties settled by the largest rounding error.
    let s = -q - r;
    let (mut rq, mut rr, rs) = (q.round(), r.round(), s.round());
    let (dq, dr, ds) = ((rq - q).abs(), (rr - r).abs(), (rs - s).abs());
    if dq > dr && dq > ds {
        rq = -rr - rs;
    } else if dr > ds {
        rr = -rq - rs;
    }
    (rq as i64, rr as i64)
}

/// The centre of a hexagon, in metres.
pub fn hex_centre(q: i64, r: i64, size: f64) -> (f64, f64) {
    let (q, r) = (q as f64, r as f64);
    (size * 3f64.sqrt() * (q + r / 2.0), size * 1.5 * r)
}

/// The closed outline of a hexagon in longitude and latitude, seven positions.
pub fn hex_ring(q: i64, r: i64, size: f64) -> Vec<[f64; 2]> {
    let (cx, cy) = hex_centre(q, r, size);
    let mut ring: Vec<[f64; 2]> = (0..6)
        .map(|i| {
            let angle = (60.0 * f64::from(i) - 30.0).to_radians();
            let (lon, lat) = to_lon_lat(cx + size * angle.cos(), cy + size * angle.sin());
            [lon, lat]
        })
        .collect();
    ring.push(ring[0]);
    ring
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn metres_round_trip_and_a_kilometre_is_a_kilometre() {
        let (x, y) = to_metres(25.0, 60.25);
        let (lon, lat) = to_lon_lat(x, y);
        assert!((lon - 25.0).abs() < 1e-9 && (lat - 60.25).abs() < 1e-9);
        let (_, north) = to_metres(ORIGIN.0, ORIGIN.1 + 1.0 / 111.195);
        assert!((north - 1000.0).abs() < 1.0, "{north}");
        assert_eq!(to_metres(ORIGIN.0, ORIGIN.1), (0.0, 0.0));
    }

    #[test]
    fn a_geometry_is_stood_for_by_the_mean_of_its_positions() {
        assert_eq!(
            representative_point(&json!({"type": "Point", "coordinates": [24.9, 60.2]})),
            Some((24.9, 60.2))
        );
        let line = json!({"type": "MultiLineString", "coordinates": [[[24.0, 60.0], [26.0, 60.0]], [[25.0, 62.0]]]});
        let (lon, lat) = representative_point(&line).expect("a point");
        assert!((lon - 25.0).abs() < 1e-12 && (lat - 60.666_666_666_666_67).abs() < 1e-9);
        assert_eq!(
            representative_point(&json!({"type": "Point", "coordinates": []})),
            None
        );
        assert_eq!(
            representative_point(&json!({"coordinates": [24.9, 60.2]})),
            None
        );
        assert_eq!(representative_point(&Value::Null), None);
        // A position outside the globe is no position.
        assert_eq!(
            representative_point(&json!({"type": "Point", "coordinates": [200.0, 60.0]})),
            None
        );
    }

    #[test]
    fn every_point_falls_in_the_hexagon_whose_centre_is_nearest() {
        let size = 500.0;
        for (x, y) in [
            (0.0, 0.0),
            (430.0, 10.0),
            (-999.0, 1234.0),
            (433.0, 250.0),
            (-0.1, -0.1),
        ] {
            let (q, r) = hex_of(x, y, size);
            let (cx, cy) = hex_centre(q, r, size);
            let mine = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
            for (dq, dr) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, -1), (-1, 1)] {
                let (nx, ny) = hex_centre(q + dq, r + dr, size);
                assert!(
                    mine <= ((x - nx).powi(2) + (y - ny).powi(2)).sqrt() + 1e-9,
                    "({x},{y}) in ({q},{r})"
                );
            }
        }
        assert_eq!(hex_of(0.0, 0.0, size), (0, 0));
    }

    #[test]
    fn a_hexagon_outline_is_closed_and_its_corners_are_one_size_away() {
        let ring = hex_ring(2, -1, 500.0);
        assert_eq!(ring.len(), 7);
        assert_eq!(ring[0], ring[6]);
        let (cx, cy) = hex_centre(2, -1, 500.0);
        for corner in &ring[..6] {
            let (x, y) = to_metres(corner[0], corner[1]);
            assert!((((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - 500.0).abs() < 1e-6);
        }
    }
}
