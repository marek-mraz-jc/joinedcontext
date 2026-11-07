// SPDX-License-Identifier: EUPL-1.2
//! Part 2 (OGC 18-058r1, mapping rows R1 to R5): `CRS84` and `CRS84h` only, because no coordinate
//! transform is in the tree (`geo` relates, it does not reproject); any other CRS is 400.

use serde_json::{json, Value};

/// `CRS84`, `GeoJSON`'s own and the storage CRS.
pub(crate) const CRS84: &str = "http://www.opengis.net/def/crs/OGC/1.3/CRS84";
/// `CRS84` with ellipsoidal height.
pub(crate) const CRS84H: &str = "http://www.opengis.net/def/crs/OGC/0/CRS84h";
/// The refusal of any other CRS: it lists the supported ones (R2).
pub(crate) const UNSUPPORTED: &str = "the supported CRS are http://www.opengis.net/def/crs/OGC/1.3/CRS84 and http://www.opengis.net/def/crs/OGC/0/CRS84h";

/// One of the supported CRS, by its URI or its safe CURIE (`[OGC:CRS84]`).
pub(crate) fn read(text: &str) -> Option<&'static str> {
    match text {
        CRS84 | "[OGC:CRS84]" => Some(CRS84),
        CRS84H | "[OGC:CRS84h]" => Some(CRS84H),
        _ => None,
    }
}

/// The corners of a CRS84 box of 4 or 6 numbers (a CQL2 `BBOX` too); heights are dropped, and a
/// number that is not finite is out of range.
pub(crate) fn corners(numbers: &[f64]) -> Option<[f64; 4]> {
    let [west, south, east, north] = match numbers {
        [west, south, east, north] => [*west, *south, *east, *north],
        [west, south, low, east, north, high] if low <= high => [*west, *south, *east, *north],
        _ => return None,
    };
    let lon = |x: f64| (-180.0..=180.0).contains(&x);
    let lat = |y: f64| (-90.0..=90.0).contains(&y);
    (lon(west) && lon(east) && lat(south) && lat(north) && south <= north)
        .then_some([west, south, east, north])
}

/// The polygon(s) of a CRS84 box; one that crosses the antimeridian (its west edge east of
/// its east edge) is two.
pub(crate) fn bbox_geometry([west, south, east, north]: [f64; 4]) -> (&'static str, Value) {
    let ring = |from: f64, to: f64| {
        json!([[
            [from, south],
            [to, south],
            [to, north],
            [from, north],
            [from, south]
        ]])
    };
    if west <= east {
        ("Polygon", ring(west, east))
    } else {
        (
            "MultiPolygon",
            json!([ring(west, 180.0), ring(-180.0, east)]),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crs84_and_crs84h_by_uri_or_curie() {
        assert_eq!(read(CRS84), Some(CRS84));
        assert_eq!(read("[OGC:CRS84h]"), Some(CRS84H));
        assert_eq!(read("http://www.opengis.net/def/crs/EPSG/0/4326"), None);
    }
}
