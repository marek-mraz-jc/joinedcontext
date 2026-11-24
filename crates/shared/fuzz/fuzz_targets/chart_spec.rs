//! Any chart spec and any rows: the subset check, the row flattening, the chart and its view never
//! panic, a refusal names a JSON pointer under the spec, the view keeps to the door's caps, and the
//! PNG has the asked sides (T-7805).
#![no_main]

use jc_chart::raster::render;
use jc_chart::spec::{Door, chart_of, look_of, rows_of, view_of};
use libfuzzer_sys::fuzz_target;
use serde_json::Value;
use std::time::{Duration, Instant};

const DOOR: Door = Door {
    tool: "render_chart",
    read: "data.arguments",
    // Small sides keep one run fast; the caps are what the fuzz checks, not the size.
    max_side: 120,
    max_height: 90,
    max_points: 200,
    max_view_points: 20,
};

fuzz_target!(|data: &[u8]| {
    // The spec as JSON, a NUL, then the read's entities as a JSON array.
    let (spec, entities) = match data.iter().position(|b| *b == 0) {
        Some(at) => (&data[..at], &data[at + 1..]),
        None => (data, &b"[]"[..]),
    };
    let Ok(Value::Object(top)) = serde_json::from_slice::<Value>(spec) else {
        return;
    };
    let entities = match serde_json::from_slice::<Value>(entities) {
        Ok(Value::Array(list)) => list,
        Ok(one) => vec![one],
        Err(_) => Vec::new(),
    };
    let spec = match look_of(&top, "/spec", (100, 80), &DOOR) {
        Ok(spec) => spec,
        Err(refusal) => {
            let (path, words) = refusal.describe(&DOOR);
            assert!(path.starts_with("/spec"), "{path}");
            assert!(!words.is_empty());
            return;
        }
    };
    assert!((50..=DOOR.max_side).contains(&spec.width));
    assert!((50..=DOOR.max_height).contains(&spec.height));
    let rows = rows_of(&entities);
    let charted = chart_of(&spec, &rows, &DOOR);
    assert!(charted.chart.points.len() <= DOOR.max_points);
    assert!(charted.view_points.len() <= DOOR.max_view_points + 1);
    let view = view_of(&spec, &charted, &DOOR);
    let shown = view["joinedcontext.com/chartPoints"]
        .as_array()
        .map_or(0, Vec::len);
    assert!(shown <= DOOR.max_view_points);
    // A fuzz input never waits: a render past its deadline is a refusal, not a hang.
    if let Ok(png) = render(&charted.chart, Instant::now() + Duration::from_secs(2)) {
        assert_eq!(&png[1..4], b"PNG");
        let side = |at: usize| u32::from_be_bytes([png[at], png[at + 1], png[at + 2], png[at + 3]]);
        assert_eq!((side(16), side(20)), (spec.width, spec.height));
    }
});
