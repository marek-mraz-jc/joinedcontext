//! Geodesic calculations around Helsinki: point-in-polygon, point-in-geometry, equirectangular
//! shoelace area in km², bounding boxes, and representative coordinate extraction (T-3335).

use serde_json::Value;

/// Mean Earth radius in kilometres (IUGG / GRS80).
pub const EARTH_RADIUS_KM: f64 = 6_371.008_8;

/// Checks whether a 2D point lies on a line segment between `a` and `b`.
pub fn is_on_segment(p: (f64, f64), a: [f64; 2], b: [f64; 2]) -> bool {
    let (px, py) = p;
    let (x1, y1) = (a[0], a[1]);
    let (x2, y2) = (b[0], b[1]);

    let min_x = x1.min(x2);
    let max_x = x1.max(x2);
    let min_y = y1.min(y2);
    let max_y = y1.max(y2);

    const EPS: f64 = 1e-10;
    if px < min_x - EPS || px > max_x + EPS || py < min_y - EPS || py > max_y + EPS {
        return false;
    }

    let cross = (px - x1) * (y2 - y1) - (py - y1) * (x2 - x1);
    cross.abs() <= EPS
}

/// Iterates the directed edges of a ring, filtering out zero-length segments and handling closure.
fn ring_segments<'a>(ring: &'a [[f64; 2]]) -> impl Iterator<Item = ([f64; 2], [f64; 2])> + 'a {
    let is_closed = ring.len() >= 2
        && (ring[0][0] - ring[ring.len() - 1][0]).abs() < 1e-12
        && (ring[0][1] - ring[ring.len() - 1][1]).abs() < 1e-12;
    let count = if is_closed {
        ring.len().saturating_sub(1)
    } else {
        ring.len()
    };
    (0..count).filter_map(move |i| {
        let next = if is_closed {
            i + 1
        } else {
            (i + 1) % ring.len()
        };
        let a = ring[i];
        let b = ring[next];
        if (a[0] - b[0]).abs() < 1e-12 && (a[1] - b[1]).abs() < 1e-12 {
            None
        } else {
            Some((a, b))
        }
    })
}

/// Even-odd point-in-polygon test: the first ring is outer, remaining rings are holes.
/// A point on any edge or vertex counts inside deterministically.
pub fn point_in_polygon<R: AsRef<[[f64; 2]]>>(point: (f64, f64), rings: &[R]) -> bool {
    if rings.is_empty() {
        return false;
    }
    let outer = rings[0].as_ref();
    if outer.len() < 3 {
        return false;
    }

    // Edge check on outer boundary
    for (a, b) in ring_segments(outer) {
        if is_on_segment(point, a, b) {
            return true;
        }
    }

    // Ray casting for outer ring
    let (px, py) = point;
    let mut inside_outer = false;
    for (a, b) in ring_segments(outer) {
        let (x1, y1) = (a[0], a[1]);
        let (x2, y2) = (b[0], b[1]);
        if (y1 > py) != (y2 > py) {
            let intersect_x = x1 + (py - y1) * (x2 - x1) / (y2 - y1);
            if px < intersect_x {
                inside_outer = !inside_outer;
            }
        }
    }
    if !inside_outer {
        return false;
    }

    // Check holes
    for hole_ring in &rings[1..] {
        let hole = hole_ring.as_ref();
        if hole.len() < 3 {
            continue;
        }
        let mut on_hole_edge = false;
        let mut inside_hole = false;
        for (a, b) in ring_segments(hole) {
            if is_on_segment(point, a, b) {
                on_hole_edge = true;
                break;
            }
            let (x1, y1) = (a[0], a[1]);
            let (x2, y2) = (b[0], b[1]);
            if (y1 > py) != (y2 > py) {
                let intersect_x = x1 + (py - y1) * (x2 - x1) / (y2 - y1);
                if px < intersect_x {
                    inside_hole = !inside_hole;
                }
            }
        }
        if on_hole_edge {
            return true;
        }
        if inside_hole {
            return false;
        }
    }

    true
}

/// Extracts rings from a GeoJSON Polygon coordinates array.
pub fn parse_polygon_rings(coords: &Value) -> Option<Vec<Vec<[f64; 2]>>> {
    let arr = coords.as_array()?;
    let mut rings = Vec::with_capacity(arr.len());
    for ring_val in arr {
        let ring_arr = ring_val.as_array()?;
        let mut ring = Vec::with_capacity(ring_arr.len());
        for pt_val in ring_arr {
            let pt_arr = pt_val.as_array()?;
            if pt_arr.len() >= 2 {
                if let (Some(lon), Some(lat)) = (pt_arr[0].as_f64(), pt_arr[1].as_f64()) {
                    if lon.is_finite() && lat.is_finite() {
                        ring.push([lon, lat]);
                    }
                }
            }
        }
        if !ring.is_empty() {
            rings.push(ring);
        }
    }
    if rings.is_empty() {
        None
    } else {
        Some(rings)
    }
}

/// Tests whether a point lies inside a GeoJSON Polygon or MultiPolygon.
pub fn point_in_geometry(point: (f64, f64), geometry: &Value) -> bool {
    let geom_type = geometry.get("type").and_then(Value::as_str);
    match geom_type {
        Some("Polygon") => {
            let coords = match geometry.get("coordinates") {
                Some(c) => c,
                None => return false,
            };
            match parse_polygon_rings(coords) {
                Some(rings) => point_in_polygon(point, &rings),
                None => false,
            }
        }
        Some("MultiPolygon") => {
            let coords = match geometry.get("coordinates").and_then(Value::as_array) {
                Some(c) => c,
                None => return false,
            };
            for poly_val in coords {
                if let Some(rings) = parse_polygon_rings(poly_val) {
                    if point_in_polygon(point, &rings) {
                        return true;
                    }
                }
            }
            false
        }
        Some("GeometryCollection") => {
            let geoms = match geometry.get("geometries").and_then(Value::as_array) {
                Some(g) => g,
                None => return false,
            };
            for g in geoms {
                if point_in_geometry(point, g) {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

/// Area in km² of a single ring, projected equirectangularly at its mean latitude.
pub fn ring_area_km2(ring: &[[f64; 2]]) -> f64 {
    let is_closed = ring.len() >= 2
        && (ring[0][0] - ring[ring.len() - 1][0]).abs() < 1e-12
        && (ring[0][1] - ring[ring.len() - 1][1]).abs() < 1e-12;
    let n = if is_closed {
        ring.len().saturating_sub(1)
    } else {
        ring.len()
    };
    if n < 3 {
        return 0.0;
    }

    let sum_lat: f64 = ring[..n].iter().map(|p| p[1]).sum();
    let mean_lat = sum_lat / (n as f64);
    let mean_lat_rad = mean_lat.to_radians();
    let cos_lat = mean_lat_rad.cos();

    let deg_to_km = EARTH_RADIUS_KM * std::f64::consts::PI / 180.0;
    let kx = deg_to_km * cos_lat;
    let ky = deg_to_km;

    let sum_lon: f64 = ring[..n].iter().map(|p| p[0]).sum();
    let mean_lon = sum_lon / (n as f64);

    let mut sum = 0.0;
    for i in 0..n {
        let j = (i + 1) % n;
        let x1 = (ring[i][0] - mean_lon) * kx;
        let y1 = (ring[i][1] - mean_lat) * ky;
        let x2 = (ring[j][0] - mean_lon) * kx;
        let y2 = (ring[j][1] - mean_lat) * ky;
        sum += x1 * y2 - x2 * y1;
    }

    sum.abs() * 0.5
}

/// Area in km² of a polygon with holes (outer minus interior holes).
pub fn polygon_area_km2(rings: &[Vec<[f64; 2]>]) -> f64 {
    if rings.is_empty() {
        return 0.0;
    }
    let outer_area = ring_area_km2(&rings[0]);
    let mut holes_area = 0.0;
    for hole in &rings[1..] {
        holes_area += ring_area_km2(hole);
    }
    (outer_area - holes_area).max(0.0)
}

/// Total area in km² of a GeoJSON Polygon or MultiPolygon.
pub fn area_km2(geometry: &Value) -> f64 {
    let geom_type = geometry.get("type").and_then(Value::as_str);
    match geom_type {
        Some("Polygon") => {
            let coords = match geometry.get("coordinates") {
                Some(c) => c,
                None => return 0.0,
            };
            match parse_polygon_rings(coords) {
                Some(rings) => polygon_area_km2(&rings),
                None => 0.0,
            }
        }
        Some("MultiPolygon") => {
            let coords = match geometry.get("coordinates").and_then(Value::as_array) {
                Some(c) => c,
                None => return 0.0,
            };
            let mut total = 0.0;
            for poly_val in coords {
                if let Some(rings) = parse_polygon_rings(poly_val) {
                    total += polygon_area_km2(&rings);
                }
            }
            total
        }
        Some("GeometryCollection") => {
            let geoms = match geometry.get("geometries").and_then(Value::as_array) {
                Some(g) => g,
                None => return 0.0,
            };
            geoms.iter().map(area_km2).sum()
        }
        _ => 0.0,
    }
}

/// Computes [min_lon, min_lat, max_lon, max_lat] bounding box of a geometry.
pub fn bbox(geometry: &Value) -> Option<[f64; 4]> {
    let mut min_lon = f64::INFINITY;
    let mut min_lat = f64::INFINITY;
    let mut max_lon = f64::NEG_INFINITY;
    let mut max_lat = f64::NEG_INFINITY;

    fn walk(
        val: &Value,
        min_lon: &mut f64,
        min_lat: &mut f64,
        max_lon: &mut f64,
        max_lat: &mut f64,
    ) {
        if let Some(arr) = val.as_array() {
            if arr.len() >= 2 && arr[0].is_number() && arr[1].is_number() {
                if let (Some(lon), Some(lat)) = (arr[0].as_f64(), arr[1].as_f64()) {
                    if lon.is_finite() && lat.is_finite() {
                        *min_lon = min_lon.min(lon);
                        *min_lat = min_lat.min(lat);
                        *max_lon = max_lon.max(lon);
                        *max_lat = max_lat.max(lat);
                    }
                }
                return;
            }
            for item in arr {
                walk(item, min_lon, min_lat, max_lon, max_lat);
            }
        }
    }

    if let Some(coords) = geometry.get("coordinates") {
        walk(
            coords,
            &mut min_lon,
            &mut min_lat,
            &mut max_lon,
            &mut max_lat,
        );
    } else if let Some(geoms) = geometry.get("geometries").and_then(Value::as_array) {
        for g in geoms {
            if let Some(b) = bbox(g) {
                min_lon = min_lon.min(b[0]);
                min_lat = min_lat.min(b[1]);
                max_lon = max_lon.max(b[2]);
                max_lat = max_lat.max(b[3]);
            }
        }
    }

    if min_lon.is_finite() && min_lat.is_finite() && max_lon.is_finite() && max_lat.is_finite() {
        Some([min_lon, min_lat, max_lon, max_lat])
    } else {
        None
    }
}

/// Checks whether a point lies within a bounding box.
pub fn bbox_contains(b: &[f64; 4], point: (f64, f64)) -> bool {
    point.0 >= b[0] - 1e-10
        && point.0 <= b[2] + 1e-10
        && point.1 >= b[1] - 1e-10
        && point.1 <= b[3] + 1e-10
}

/// Extracts a representative point: a Point's coordinates, else the first coordinate.
pub fn place_of(geometry: &Value) -> Option<(f64, f64)> {
    fn first_coord(val: &Value) -> Option<(f64, f64)> {
        let arr = val.as_array()?;
        if arr.len() >= 2 && arr[0].is_number() && arr[1].is_number() {
            let lon = arr[0].as_f64()?;
            let lat = arr[1].as_f64()?;
            if lon.is_finite()
                && lat.is_finite()
                && (-180.0..=180.0).contains(&lon)
                && (-90.0..=90.0).contains(&lat)
            {
                return Some((lon, lat));
            }
            return None;
        }
        for item in arr {
            if let Some(pt) = first_coord(item) {
                return Some(pt);
            }
        }
        None
    }

    let geom_type = geometry.get("type").and_then(Value::as_str);
    if geom_type == Some("Point") {
        let coords = geometry.get("coordinates")?;
        let arr = coords.as_array()?;
        if arr.len() >= 2 {
            let lon = arr[0].as_f64()?;
            let lat = arr[1].as_f64()?;
            if lon.is_finite()
                && lat.is_finite()
                && (-180.0..=180.0).contains(&lon)
                && (-90.0..=90.0).contains(&lat)
            {
                return Some((lon, lat));
            }
        }
        return None;
    }

    if let Some(coords) = geometry.get("coordinates") {
        return first_coord(coords);
    }
    if let Some(geoms) = geometry.get("geometries").and_then(Value::as_array) {
        for g in geoms {
            if let Some(pt) = place_of(g) {
                return Some(pt);
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn square_and_square_with_hole() {
        let square = vec![vec![
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 10.0],
            [0.0, 10.0],
            [0.0, 0.0],
        ]];
        assert!(point_in_polygon((5.0, 5.0), &square));
        assert!(!point_in_polygon((15.0, 5.0), &square));
        assert!(!point_in_polygon((-1.0, 5.0), &square));

        let square_with_hole = vec![
            vec![
                [0.0, 0.0],
                [10.0, 0.0],
                [10.0, 10.0],
                [0.0, 10.0],
                [0.0, 0.0],
            ],
            vec![[3.0, 3.0], [7.0, 3.0], [7.0, 7.0], [3.0, 7.0], [3.0, 3.0]],
        ];
        // In outer, outside hole
        assert!(point_in_polygon((1.0, 1.0), &square_with_hole));
        // Inside hole
        assert!(!point_in_polygon((5.0, 5.0), &square_with_hole));
    }

    #[test]
    fn point_on_edge_and_on_vertex() {
        let square = vec![vec![
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 10.0],
            [0.0, 10.0],
            [0.0, 0.0],
        ]];
        // On edge
        assert!(point_in_polygon((0.0, 5.0), &square));
        assert!(point_in_polygon((5.0, 0.0), &square));
        assert!(point_in_polygon((10.0, 5.0), &square));
        // On vertex
        assert!(point_in_polygon((0.0, 0.0), &square));
        assert!(point_in_polygon((10.0, 10.0), &square));

        let square_with_hole = vec![
            vec![
                [0.0, 0.0],
                [10.0, 0.0],
                [10.0, 10.0],
                [0.0, 10.0],
                [0.0, 0.0],
            ],
            vec![[2.0, 2.0], [8.0, 2.0], [8.0, 8.0], [2.0, 8.0], [2.0, 2.0]],
        ];
        // On hole edge
        assert!(point_in_polygon((2.0, 5.0), &square_with_hole));
        // On hole vertex
        assert!(point_in_polygon((2.0, 2.0), &square_with_hole));
    }

    #[test]
    fn multipolygon() {
        let geom = json!({
            "type": "MultiPolygon",
            "coordinates": [
                [[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0], [0.0, 0.0]]],
                [[[20.0, 20.0], [30.0, 20.0], [30.0, 30.0], [20.0, 30.0], [20.0, 20.0]]]
            ]
        });
        assert!(point_in_geometry((5.0, 5.0), &geom));
        assert!(point_in_geometry((25.0, 25.0), &geom));
        assert!(!point_in_geometry((15.0, 15.0), &geom));
    }

    #[test]
    fn area_of_known_1km_square_near_helsinki() {
        // At latitude 60.1699 near Helsinki centre
        let lat: f64 = 60.1699;
        let lon = 24.9384;
        let deg_to_km = EARTH_RADIUS_KM * std::f64::consts::PI / 180.0;
        let d_lat = 1.0 / deg_to_km;
        let d_lon = 1.0 / (deg_to_km * lat.to_radians().cos());

        let geom = json!({
            "type": "Polygon",
            "coordinates": [
                [
                    [lon, lat],
                    [lon + d_lon, lat],
                    [lon + d_lon, lat + d_lat],
                    [lon, lat + d_lat],
                    [lon, lat]
                ]
            ]
        });
        let area = area_km2(&geom);
        assert!((area - 1.0).abs() < 0.01, "Expected ~1.00 km², got {area}");
    }

    #[test]
    fn bbox_and_place_of() {
        let point_geom = json!({ "type": "Point", "coordinates": [24.94, 60.17] });
        assert_eq!(place_of(&point_geom), Some((24.94, 60.17)));
        assert_eq!(bbox(&point_geom), Some([24.94, 60.17, 24.94, 60.17]));

        let line_geom = json!({
            "type": "LineString",
            "coordinates": [[24.90, 60.15], [25.00, 60.20]]
        });
        assert_eq!(place_of(&line_geom), Some((24.90, 60.15)));
        assert_eq!(bbox(&line_geom), Some([24.90, 60.15, 25.00, 60.20]));

        assert_eq!(
            place_of(&json!({ "type": "Point", "coordinates": [] })),
            None
        );
        assert_eq!(bbox(&json!({ "type": "Polygon", "coordinates": [] })), None);
    }

    fn square(x: f64) -> Value {
        json!([[[x, 0.0], [x + 1.0, 0.0], [x + 1.0, 1.0], [x, 1.0], [x, 0.0]]])
    }

    #[test]
    fn a_multipolygon_and_a_collection_hold_their_parts_and_add_their_areas() {
        let multi = json!({ "type": "MultiPolygon", "coordinates": [square(0.0), square(5.0)] });
        let collection = json!({ "type": "GeometryCollection", "geometries": [{ "type": "Polygon", "coordinates": square(0.0) }, { "type": "Polygon", "coordinates": square(5.0) }] });
        for geometry in [&multi, &collection] {
            assert!(point_in_geometry((5.5, 0.5), geometry));
            assert!(!point_in_geometry((3.0, 0.5), geometry));
        }
        let one = area_km2(&json!({ "type": "Polygon", "coordinates": square(0.0) }));
        assert!(one > 0.0);
        assert!((area_km2(&multi) - 2.0 * one).abs() < one * 0.01);
        assert!((area_km2(&collection) - area_km2(&multi)).abs() < 1e-9);
        assert_eq!(bbox(&collection), Some([0.0, 0.0, 6.0, 1.0]));
        assert!(place_of(&collection).is_some());
    }

    #[test]
    fn a_hole_is_taken_out_of_the_area() {
        let with_hole = json!({ "type": "Polygon", "coordinates": [
            [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0], [0.0, 0.0]],
            [[1.0, 1.0], [2.0, 1.0], [2.0, 2.0], [1.0, 2.0], [1.0, 1.0]]
        ] });
        let whole = json!({ "type": "Polygon", "coordinates": [[[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0], [0.0, 0.0]]] });
        assert!(area_km2(&with_hole) < area_km2(&whole));
    }

    #[test]
    fn a_geometry_without_its_parts_holds_nothing_and_has_no_area() {
        for broken in [
            json!({ "type": "Polygon" }),
            json!({ "type": "MultiPolygon" }),
            json!({ "type": "GeometryCollection" }),
            json!({ "type": "LineString", "coordinates": [[0.0, 0.0], [1.0, 1.0]] }),
            json!({}),
        ] {
            assert!(!point_in_geometry((0.5, 0.5), &broken), "{broken}");
            assert_eq!(area_km2(&broken), 0.0, "{broken}");
        }
        assert_eq!(
            place_of(&json!({ "type": "GeometryCollection", "geometries": [] })),
            None
        );
    }
}
