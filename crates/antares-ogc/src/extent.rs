// SPDX-License-Identifier: EUPL-1.2
//! C6 and M8: a collection's spatial `extent` (T-5909), the bounding box of the Features the
//! caller reads through the view. It is read as the caller through [`items::page`], so the
//! view's rules, its `q` and the Policy narrow it exactly as they narrow `items`: a hidden
//! Entity's location never widens it. One read takes at most [`EXTENT_SCAN`] Features, with no
//! whole count (T-6508: on dev that count alone outran the read cut of a type of 67 000 rows,
//! a page of it took 1 s); a collection filling that page, a history collection and one
//! without a geometry state none.
//! An answer is kept for [`FRESH_MICROS`] per view compilation, collection and caller, so a
//! `/collections` read costs one read per collection per minute and `/collections` and
//! `/collections/{c}` state the same extent (ETS `FeatureCollection` compares them).

use std::any::Any;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, PoisonError, Weak};

use antares_ops::{Caller, ViewOps};
use serde_json::{json, Value};

use crate::items::{self, Params, Profile, Refs};
use crate::model::Collection;

/// The most Features one extent read takes: one upstream page of the broker.
// ponytail: a larger collection states no extent; a store aggregate (ST_Extent under the
// view's rules) when a deployment needs the extent of one.
pub(crate) const EXTENT_SCAN: u64 = 1_000;

/// How long a computed extent is served before it is read again.
const FRESH_MICROS: i64 = 60_000_000;

/// (Tenant, view, collection, the caller's policy subject).
type Key = (String, String, String, Vec<(String, Vec<u8>)>);

struct Kept {
    /// The view compilation the extent was read through: a rule change makes a new one.
    view: Weak<dyn Any + Send + Sync>,
    at: i64,
    bbox: Option<[f64; 4]>,
}

static CACHE: LazyLock<Mutex<HashMap<Key, Kept>>> = LazyLock::new(Mutex::default);

/// The `extent` member of collection `c` for this caller, `None` when it states none.
pub(crate) async fn of(
    view: &ViewOps,
    view_id: &str,
    caller: &Caller,
    c: &Collection,
) -> Option<Value> {
    if c.history || c.geometry.is_none() {
        return None;
    }
    let now = items::now()?.unix_micros();
    let key = (
        view.tenant().to_owned(),
        view_id.to_owned(),
        c.id.clone(),
        caller.subject(),
    );
    let generation = view.generation();
    let cached = CACHE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
        .filter(|k| Weak::ptr_eq(&k.view, &generation) && now.saturating_sub(k.at) < FRESH_MICROS)
        .map(|k| k.bbox);
    let bbox = if let Some(bbox) = cached {
        bbox
    } else {
        let bbox = read(view, caller, c).await;
        let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
        // ponytail: stale extents go on each insert, so the cache holds one minute of keys.
        cache.retain(|_, k| now.saturating_sub(k.at) < FRESH_MICROS);
        cache.insert(
            key,
            Kept {
                view: generation,
                at: now,
                bbox,
            },
        );
        bbox
    };
    bbox.map(|b| json!({"spatial": {"bbox": [b], "crs": crate::crs::CRS84}}))
}

/// The bounding box of the collection's Features as the caller reads them; `None` at
/// [`EXTENT_SCAN`] Features, on a refused read, or when no Feature has a position.
async fn read(view: &ViewOps, caller: &Caller, c: &Collection) -> Option<[f64; 4]> {
    let p = Params {
        limit: EXTENT_SCAN,
        properties: Some(Vec::new()),
        uncounted: true,
        ..items::params(c, &[]).ok()?
    };
    let refs = Refs {
        profile: Profile::default(),
        base: "",
        alternate: false,
    };
    let page = items::page(view, caller, c, &p, 0, refs).await.ok()?;
    if page.features.len() as u64 >= EXTENT_SCAN || page.next.is_some() {
        return None;
    }
    page.features
        .iter()
        .filter_map(|f| f.get("geometry"))
        .filter_map(bbox)
        .reduce(widen)
}

/// The 2D bounding box of a `GeoJSON` geometry, `None` for a null or empty one.
fn bbox(geometry: &Value) -> Option<[f64; 4]> {
    if let Some(parts) = geometry.get("geometries").and_then(Value::as_array) {
        return parts.iter().filter_map(bbox).reduce(widen);
    }
    positions(geometry.get("coordinates")?)
}

fn positions(v: &Value) -> Option<[f64; 4]> {
    let items = v.as_array()?;
    match items.as_slice() {
        [x, y, ..] if x.is_number() => {
            let (x, y) = (x.as_f64()?, y.as_f64()?);
            Some([x, y, x, y])
        }
        _ => items.iter().filter_map(positions).reduce(widen),
    }
}

fn widen(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_geometry_bounds_every_position() {
        let point = json!({"type": "Point", "coordinates": [17.1, 48.1]});
        assert_eq!(bbox(&point), Some([17.1, 48.1, 17.1, 48.1]));
        let polygon = json!({"type": "Polygon", "coordinates": [[[0.0, 1.0], [2.0, -1.0], [1.0, 3.0], [0.0, 1.0]]]});
        assert_eq!(bbox(&polygon), Some([0.0, -1.0, 2.0, 3.0]));
        let both = json!({"type": "GeometryCollection", "geometries": [point, polygon]});
        assert_eq!(bbox(&both), Some([0.0, -1.0, 17.1, 48.1]));
        assert_eq!(bbox(&Value::Null), None);
        assert_eq!(bbox(&json!({"type": "Point", "coordinates": []})), None);
    }
}
