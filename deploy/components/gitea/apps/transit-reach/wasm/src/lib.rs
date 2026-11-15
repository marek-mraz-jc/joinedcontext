//! How far one gets by transit from any point of Helsinki in 10, 20 and 30 minutes (T-3331,
//! T-3356), computed in the visitor's browser: HSL's stops and lines when the space holds them,
//! else stops and rides derived from where and when vehicles stood still; then a walk to a stop, a
//! wait, rides and changes, and a walk on, onto a hexagon grid. One call, JSON in and JSON out,
//! from a Web Worker.

pub mod dbscan;
pub mod geo;
pub mod network;
pub mod reach;
pub mod stops;

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
#[cfg(feature = "web")]
use wasm_bindgen::prelude::wasm_bindgen;

use network::{build, Label, Network};
use reach::{hexes_reached, stops_reached, Rules};
use stops::{derive, Reading};

/// The median of the values, or `None` for none.
pub fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    Some(if sorted.len().is_multiple_of(2) {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Point {
    /// Milliseconds since the epoch.
    pub t: f64,
    pub lon: f64,
    pub lat: f64,
    /// Metres a second; absent counts as moving.
    #[serde(default)]
    pub speed: Option<f64>,
    #[serde(default)]
    pub route: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Vehicle {
    pub id: String,
    #[serde(default)]
    pub points: Vec<Point>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LonLat {
    pub lon: f64,
    pub lat: f64,
}

fn default_bands() -> Vec<f64> {
    vec![10.0, 20.0, 30.0]
}
fn default_wait() -> f64 {
    5.0
}
fn default_hex() -> f64 {
    150.0
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Input {
    #[serde(default)]
    pub vehicles: Vec<Vehicle>,
    /// HSL's stops and lines; used over the vehicles when it holds a stop and a line.
    #[serde(default)]
    pub network: Option<Network>,
    pub origin: LonLat,
    /// The minutes each band ends at, ascending; the last is how far the search looks.
    #[serde(default = "default_bands")]
    pub bands: Vec<f64>,
    /// Minutes waited at each boarding.
    #[serde(default = "default_wait")]
    pub wait: f64,
    /// The hexagons' circumradius in metres.
    #[serde(default = "default_hex")]
    pub hex_size: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StopOut {
    pub lon: f64,
    pub lat: f64,
    /// The stop's name and sign code, from HSL's register; none for a derived stop.
    pub name: Option<String>,
    pub code: Option<String>,
    pub vehicles: usize,
    pub routes: Vec<String>,
    /// When it is reached on foot from the point; `None` beyond the last band.
    pub minutes: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct Cell {
    pub ring: Vec<[f64; 2]>,
    pub minutes: f64,
    /// The band it falls in: the first band's minutes at or above its own.
    pub band: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Band {
    pub minutes: f64,
    /// Everything reached within it, the earlier bands included.
    pub area_km2: f64,
    pub stops: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Output {
    pub origin: LonLat,
    /// What the stops and rides came from: `network` (HSL's registers) or `vehicles`.
    pub source: &'static str,
    pub vehicles: usize,
    pub readings: usize,
    pub first: Option<f64>,
    pub last: Option<f64>,
    pub stops: Vec<StopOut>,
    pub rides: usize,
    pub routes: Vec<String>,
    pub cells: Vec<Cell>,
    pub bands: Vec<Band>,
}

fn readings_of(vehicle: &Vehicle) -> Vec<Reading> {
    vehicle
        .points
        .iter()
        .filter(|p| {
            p.t.is_finite() && (-180.0..=180.0).contains(&p.lon) && (-90.0..=90.0).contains(&p.lat)
        })
        .map(|p| {
            let (x, y) = geo::to_metres(p.lon, p.lat);
            Reading {
                t: p.t,
                x,
                y,
                speed: p.speed.filter(|s| s.is_finite()).unwrap_or(f64::INFINITY),
                route: p.route.clone().unwrap_or_default(),
            }
        })
        .collect()
}

pub fn run(input: &Input) -> Result<Output, String> {
    let LonLat { lon, lat } = input.origin;
    if !((-180.0..=180.0).contains(&lon) && (-90.0..=90.0).contains(&lat)) {
        return Err(format!("the point {lon}, {lat} is not on the globe"));
    }
    let mut bands: Vec<f64> = input
        .bands
        .iter()
        .copied()
        .filter(|b| b.is_finite() && *b > 0.0)
        .collect();
    bands.sort_by(f64::total_cmp);
    bands.dedup();
    let Some(&limit) = bands.last() else {
        return Err("no band to reach within".into());
    };
    if !(input.hex_size.is_finite()
        && input.hex_size >= 20.0
        && input.wait.is_finite()
        && input.wait >= 0.0)
    {
        return Err("the hexagon size or the wait is out of range".into());
    }

    let vehicles: Vec<Vec<Reading>> = input.vehicles.iter().map(readings_of).collect();
    let times: Vec<f64> = vehicles.iter().flatten().map(|r| r.t).collect();
    let (stops, labels, rides, source) = match input.network.as_ref().map(build) {
        Some((stops, labels, rides)) if !stops.is_empty() && !rides.is_empty() => {
            (stops, labels, rides, "network")
        }
        _ => {
            let (stops, rides) = derive(&vehicles);
            let labels = vec![
                Label {
                    name: None,
                    code: None
                };
                stops.len()
            ];
            (stops, labels, rides, "vehicles")
        }
    };
    let origin = geo::to_metres(lon, lat);
    let at = stops_reached(
        origin,
        &stops,
        &rides,
        Rules {
            wait: input.wait,
            limit,
        },
    );
    let hexes = hexes_reached(origin, &stops, &at, input.hex_size, limit);

    let band_of = |minutes: f64| {
        bands
            .iter()
            .copied()
            .find(|b| minutes <= *b)
            .unwrap_or(limit)
    };
    let mut keys: Vec<&(i64, i64)> = hexes.keys().collect();
    keys.sort();
    let cells: Vec<Cell> = keys
        .into_iter()
        .map(|key| {
            let minutes = hexes[key];
            Cell {
                ring: geo::hex_ring(key.0, key.1, input.hex_size),
                minutes: (minutes * 10.0).round() / 10.0,
                band: band_of(minutes),
            }
        })
        .collect();
    let hexagon_km2 = 1.5 * 3f64.sqrt() * input.hex_size * input.hex_size / 1e6;
    let summary = bands
        .iter()
        .map(|&b| Band {
            minutes: b,
            area_km2: hexes.values().filter(|m| **m <= b).count() as f64 * hexagon_km2,
            stops: at.iter().filter(|m| m.is_some_and(|m| m <= b)).count(),
        })
        .collect();
    let routes: BTreeSet<String> = rides.iter().map(|r| r.route.clone()).collect();
    Ok(Output {
        origin: input.origin,
        source,
        vehicles: vehicles.iter().filter(|v| !v.is_empty()).count(),
        readings: times.len(),
        first: times.iter().copied().reduce(f64::min),
        last: times.iter().copied().reduce(f64::max),
        stops: stops
            .iter()
            .zip(&at)
            .zip(labels)
            .map(|((s, m), label)| StopOut {
                lon: s.lon,
                lat: s.lat,
                name: label.name,
                code: label.code,
                vehicles: s.vehicles,
                routes: s.routes.clone(),
                minutes: m.map(|m| (m * 10.0).round() / 10.0),
            })
            .collect(),
        rides: rides.len(),
        routes: routes.into_iter().collect(),
        cells,
        bands: summary,
    })
}

/// The module's one entry: the input as JSON, the output as JSON, or `{"error": …}`.
#[cfg_attr(feature = "web", wasm_bindgen)]
pub fn analyse(input: &str) -> String {
    let answer = serde_json::from_str::<Input>(input)
        .map_err(|e| format!("the vehicles could not be read: {e}"))
        .and_then(|parsed| run(&parsed));
    match answer {
        Ok(out) => serde_json::to_string(&out)
            .unwrap_or_else(|e| serde_json::json!({ "error": e.to_string() }).to_string()),
        Err(e) => serde_json::json!({ "error": e }).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    const MIN: f64 = 60_000.0;
    /// Helsinki's centre, and 4 km east of it along the same parallel.
    const A: (f64, f64) = (24.9384, 60.1699);

    fn east(metres: f64) -> f64 {
        geo::to_lon_lat(metres, 0.0).0
    }

    /// A vehicle standing at the centre, riding 6 minutes, standing 4 km east.
    fn vehicle(id: &str, start: f64) -> Value {
        let p = |t: f64, x: f64, speed: f64| json!({ "t": start + t * MIN, "lon": east(x), "lat": A.1, "speed": speed, "route": "550" });
        json!({ "id": id, "points": [
            p(0.0, 0.0, 0.0), p(0.25, 3.0, 0.0), p(0.5, 1.0, 0.0), p(2.0, 1500.0, 10.0),
            p(6.5, 4000.0, 0.0), p(6.75, 4002.0, 0.0), p(7.0, 3999.0, 0.0)
        ]})
    }

    fn answer(input: Value) -> Value {
        serde_json::from_str(&analyse(&input.to_string())).expect("json")
    }

    #[test]
    fn two_runs_make_a_line_and_the_far_stop_is_reached_by_riding() {
        let out = answer(
            json!({ "vehicles": [vehicle("a", 0.0), vehicle("b", 20.0 * MIN)], "origin": { "lon": A.0, "lat": A.1 } }),
        );
        assert_eq!(out["stops"].as_array().map(Vec::len), Some(2));
        assert_eq!(out["rides"], 1);
        assert_eq!(out["routes"], json!(["550"]));
        // On foot at the first stop at once, 5 min wait, 6 min ride.
        assert_eq!(out["stops"][1]["minutes"], 11.0);
        let bands = out["bands"].as_array().expect("bands");
        assert_eq!(
            bands
                .iter()
                .map(|b| b["stops"].as_u64().unwrap_or(0))
                .collect::<Vec<_>>(),
            vec![1, 2, 2]
        );
        let areas: Vec<f64> = bands
            .iter()
            .map(|b| b["areaKm2"].as_f64().unwrap_or(0.0))
            .collect();
        assert!(
            areas[0] > 0.0 && areas[0] < areas[1] && areas[1] < areas[2],
            "{areas:?}"
        );
        assert!(out["cells"]
            .as_array()
            .expect("cells")
            .iter()
            .all(|c| c["minutes"].as_f64().unwrap_or(99.0) <= 30.0));
        assert_eq!(out["vehicles"], 2);
        assert_eq!(out["readings"], 14);
    }

    #[test]
    fn hsls_stops_and_lines_are_used_over_the_vehicles_and_name_the_stops() {
        let stop = |id: &str, x: f64, name: &str, code: &str| json!({ "id": id, "lon": east(x), "lat": A.1, "name": name, "code": code });
        let out = answer(json!({
            "vehicles": [vehicle("a", 0.0), vehicle("b", 20.0 * MIN)],
            "network": {
                "stops": [stop("1", 0.0, "Rautatientori", "H0019"), stop("2", 4000.0, "Kalasatama", "H0026")],
                "routes": [{ "name": "M1", "mode": "metro", "stops": ["1", "2"] }]
            },
            "origin": { "lon": A.0, "lat": A.1 }
        }));
        assert_eq!(out["source"], "network");
        assert_eq!(out["stops"][1]["name"], "Kalasatama");
        assert_eq!(out["stops"][1]["code"], "H0026");
        assert_eq!(out["stops"][1]["routes"], json!(["M1"]));
        // 5 min wait, 4 km × 1.2 at 40 km/h is 7.2 min.
        assert_eq!(out["stops"][1]["minutes"], 12.2);
        assert_eq!(out["routes"], json!(["M1"]));
    }

    #[test]
    fn an_empty_network_falls_back_to_the_vehicles() {
        let out = answer(json!({
            "vehicles": [vehicle("a", 0.0), vehicle("b", 20.0 * MIN)],
            "network": { "stops": [], "routes": [] },
            "origin": { "lon": A.0, "lat": A.1 }
        }));
        assert_eq!(out["source"], "vehicles");
        assert_eq!(out["stops"].as_array().map(Vec::len), Some(2));
        assert_eq!(out["stops"][0]["name"], Value::Null);
    }

    #[test]
    fn without_vehicles_the_answer_is_the_walk_alone() {
        let out = answer(json!({ "vehicles": [], "origin": { "lon": A.0, "lat": A.1 } }));
        assert_eq!(out["stops"], json!([]));
        assert_eq!(out["rides"], 0);
        assert!(
            out["bands"][2]["areaKm2"].as_f64().expect("an area") > 5.0,
            "a 30-minute walk covers some 9 km²"
        );
        assert_eq!(out["first"], Value::Null);
    }

    #[test]
    fn unreadable_points_are_dropped_and_a_missing_speed_is_moving() {
        let out = answer(json!({
            "vehicles": [{ "id": "x", "points": [
                { "t": 0, "lon": 400.0, "lat": 60.0, "speed": 0 },
                { "t": 1, "lon": 24.9, "lat": 60.2 },
                { "t": 2, "lon": 24.9, "lat": 60.2, "speed": 0, "route": "1" }
            ]}],
            "origin": { "lon": A.0, "lat": A.1 }
        }));
        assert_eq!(out["readings"], 2);
        assert_eq!(out["stops"], json!([]), "one vehicle is no stop");
    }

    #[test]
    fn bands_are_sorted_and_a_wrong_input_is_an_error() {
        let out = answer(
            json!({ "vehicles": [], "origin": { "lon": A.0, "lat": A.1 }, "bands": [20, 5, 20, -1] }),
        );
        assert_eq!(
            out["bands"]
                .as_array()
                .map(|b| b.iter().map(|x| x["minutes"].clone()).collect::<Vec<_>>()),
            Some(vec![json!(5.0), json!(20.0)])
        );
        assert!(answer(
            json!({ "vehicles": [], "origin": { "lon": A.0, "lat": A.1 }, "bands": [] })
        )["error"]
            .is_string());
        assert!(
            answer(json!({ "vehicles": [], "origin": { "lon": 200.0, "lat": 0.0 } }))["error"]
                .is_string()
        );
        assert!(
            answer(json!({ "vehicles": [], "origin": { "lon": 0, "lat": 0 }, "hexSize": 1 }))
                ["error"]
                .is_string()
        );
        assert!(answer(json!({ "vehicles": 3 }))["error"]
            .as_str()
            .expect("an error")
            .starts_with("the vehicles could not be read"));
        assert!(
            answer(json!({ "vehicles": [], "origin": { "lon": 0, "lat": 0 }, "extra": 1 }))
                ["error"]
                .is_string()
        );
    }

    #[test]
    fn the_median_of_none_one_odd_and_even() {
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 1.0, 3.0, 2.0]), Some(2.5));
    }
}
