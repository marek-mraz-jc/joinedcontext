//! Where and when Helsinki's alerts happen (T-3333), computed in the visitor's browser: every alert
//! the filters keep, binned into hexagons, the places alerts keep coming back to (DBSCAN), and how
//! they fall over the hours of the week on Helsinki's clock. One call, JSON in and JSON out, from
//! a Web Worker so the page never waits on it.

pub mod dbscan;
pub mod geo;
pub mod time;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(feature = "browser")]
use wasm_bindgen::prelude::wasm_bindgen;

/// One alert as the page hands it over: the geometry as the endpoint answers it, and the time in
/// milliseconds since the epoch (the page parses the date; `None` when the alert carries none).
#[derive(Debug, Deserialize)]
pub struct Alert {
    pub id: String,
    #[serde(default)]
    pub geometry: Value,
    #[serde(default)]
    pub time: Option<f64>,
    #[serde(default, rename = "subCategory")]
    pub sub_category: Option<String>,
}

/// What the reader narrowed the view to; every field absent keeps everything.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Filter {
    pub from: Option<f64>,
    pub to: Option<f64>,
    /// The sub-categories kept; empty keeps all.
    pub sub_categories: Vec<String>,
    /// Weekday (Monday 0) and hour of the week kept; absent keeps every hour.
    pub weekday: Option<u32>,
    pub hour: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    pub alerts: Vec<Alert>,
    #[serde(default)]
    pub filter: Filter,
    /// The hexagon circumradius in metres.
    #[serde(default = "default_hex")]
    pub hex_size: f64,
    /// DBSCAN's radius in metres and the alerts a repeat place holds at least.
    #[serde(default = "default_eps")]
    pub eps: f64,
    #[serde(default = "default_min_points")]
    pub min_points: usize,
}

fn default_hex() -> f64 {
    600.0
}
fn default_eps() -> f64 {
    120.0
}
fn default_min_points() -> usize {
    3
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Hex {
    pub id: String,
    pub count: usize,
    pub lon: f64,
    pub lat: f64,
    pub ring: Vec<[f64; 2]>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Place {
    pub count: usize,
    pub lon: f64,
    pub lat: f64,
    /// How far, in metres, the farthest alert of the place lies from its centre.
    pub radius: f64,
    pub ids: Vec<String>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Busiest {
    pub weekday: u32,
    pub hour: u32,
    pub count: usize,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Count {
    pub name: String,
    pub count: usize,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Output {
    /// Every alert handed over, the ones the filters keep, and those of them with no place or time.
    pub total: usize,
    pub kept: usize,
    pub unlocated: usize,
    pub untimed: usize,
    pub hexes: Vec<Hex>,
    pub places: Vec<Place>,
    /// Seven rows, Monday first, of 24 hours on Helsinki's clock.
    pub hour_of_week: Vec<Vec<usize>>,
    pub busiest: Option<Busiest>,
    /// Every sub-category of the alerts in the time range, so a filter never hides its own choices.
    pub sub_categories: Vec<Count>,
    pub first: Option<f64>,
    pub last: Option<f64>,
}

fn in_range(alert: &Alert, filter: &Filter) -> bool {
    match alert.time {
        Some(t) => filter.from.is_none_or(|from| t >= from) && filter.to.is_none_or(|to| t < to),
        // An alert with no time is kept only while no time range is asked for.
        None => filter.from.is_none() && filter.to.is_none(),
    }
}

/// The whole analysis, as plain Rust: tested on the host, called from `analyse` in the browser.
pub fn run(input: &Input) -> Output {
    let filter = &input.filter;
    let ranged: Vec<&Alert> = input
        .alerts
        .iter()
        .filter(|a| in_range(a, filter))
        .collect();

    let mut categories: BTreeMap<String, usize> = BTreeMap::new();
    for alert in &ranged {
        *categories
            .entry(alert.sub_category.clone().unwrap_or_default())
            .or_default() += 1;
    }
    let wanted: BTreeSet<&str> = filter.sub_categories.iter().map(String::as_str).collect();
    let kept: Vec<&Alert> = ranged
        .into_iter()
        .filter(|a| wanted.is_empty() || wanted.contains(a.sub_category.as_deref().unwrap_or("")))
        .filter(|a| match (filter.weekday, filter.hour, a.time) {
            (None, None, _) => true,
            (_, _, None) => false,
            (day, hour, Some(t)) => {
                let (d, h) = time::helsinki_weekday_hour(t as i64);
                day.is_none_or(|w| w == d) && hour.is_none_or(|x| x == h)
            }
        })
        .collect();

    let mut hour_of_week = vec![vec![0usize; 24]; 7];
    let mut untimed = 0;
    let (mut first, mut last): (Option<f64>, Option<f64>) = (None, None);
    for alert in &kept {
        match alert.time {
            Some(t) => {
                let (d, h) = time::helsinki_weekday_hour(t as i64);
                hour_of_week[d as usize][h as usize] += 1;
                first = Some(first.map_or(t, |f| f.min(t)));
                last = Some(last.map_or(t, |l| l.max(t)));
            }
            None => untimed += 1,
        }
    }
    let busiest = hour_of_week
        .iter()
        .enumerate()
        .flat_map(|(d, row)| row.iter().enumerate().map(move |(h, c)| (d, h, *c)))
        .filter(|(_, _, c)| *c > 0)
        // The most alerts; on a tie, the earliest in the week.
        .max_by(|a, b| a.2.cmp(&b.2).then(b.0.cmp(&a.0)).then(b.1.cmp(&a.1)))
        .map(|(d, h, count)| Busiest {
            weekday: d as u32,
            hour: h as u32,
            count,
        });

    let size = if input.hex_size > 0.0 {
        input.hex_size
    } else {
        default_hex()
    };
    let mut located: Vec<(&Alert, (f64, f64))> = Vec::new();
    let mut bins: HashMap<(i64, i64), usize> = HashMap::new();
    for alert in &kept {
        if let Some((lon, lat)) = geo::representative_point(&alert.geometry) {
            let metres = geo::to_metres(lon, lat);
            *bins
                .entry(geo::hex_of(metres.0, metres.1, size))
                .or_default() += 1;
            located.push((alert, metres));
        }
    }
    let mut hexes: Vec<Hex> = bins
        .into_iter()
        .map(|((q, r), count)| {
            let (cx, cy) = geo::hex_centre(q, r, size);
            let (lon, lat) = geo::to_lon_lat(cx, cy);
            Hex {
                id: format!("{q},{r}"),
                count,
                lon,
                lat,
                ring: geo::hex_ring(q, r, size),
            }
        })
        .collect();
    hexes.sort_by(|a, b| b.count.cmp(&a.count).then(a.id.cmp(&b.id)));

    let points: Vec<(f64, f64)> = located.iter().map(|(_, p)| *p).collect();
    let labels = dbscan::dbscan(&points, input.eps, input.min_points.max(1));
    let mut members: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, label) in labels.iter().enumerate() {
        if let Some(id) = label {
            members.entry(*id).or_default().push(i);
        }
    }
    let mut places: Vec<Place> = members
        .values()
        .map(|indexes| {
            let n = indexes.len() as f64;
            let (sx, sy) = indexes
                .iter()
                .fold((0.0, 0.0), |(a, b), &i| (a + points[i].0, b + points[i].1));
            let (cx, cy) = (sx / n, sy / n);
            let radius = indexes
                .iter()
                .map(|&i| ((points[i].0 - cx).powi(2) + (points[i].1 - cy).powi(2)).sqrt())
                .fold(0.0, f64::max);
            let (lon, lat) = geo::to_lon_lat(cx, cy);
            let mut ids: Vec<String> = indexes.iter().map(|&i| located[i].0.id.clone()).collect();
            ids.sort();
            Place {
                count: indexes.len(),
                lon,
                lat,
                radius: radius.round(),
                ids,
            }
        })
        .collect();
    places.sort_by(|a, b| b.count.cmp(&a.count).then(a.ids.cmp(&b.ids)));

    let mut sub_categories: Vec<Count> = categories
        .into_iter()
        .map(|(name, count)| Count { name, count })
        .collect();
    sub_categories.sort_by(|a, b| b.count.cmp(&a.count).then(a.name.cmp(&b.name)));

    Output {
        total: input.alerts.len(),
        kept: kept.len(),
        unlocated: kept.len() - located.len(),
        untimed,
        hexes,
        places,
        hour_of_week,
        busiest,
        sub_categories,
        first,
        last,
    }
}

/// The browser's door: the input as JSON, the output as JSON, or `{"error": …}` for input that is
/// not the shape above, so the page says what went wrong instead of the worker dying.
#[cfg_attr(feature = "browser", wasm_bindgen)]
pub fn analyse(input: &str) -> String {
    match serde_json::from_str::<Input>(input) {
        Ok(parsed) => serde_json::to_string(&run(&parsed))
            .unwrap_or_else(|e| serde_json::json!({ "error": e.to_string() }).to_string()),
        Err(e) => {
            serde_json::json!({ "error": format!("the alerts could not be read: {e}") }).to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ms(year: i64, month: u32, day: u32, hour: i64) -> f64 {
        (time::days_from_civil(year, month, day) * 86_400_000 + hour * 3_600_000) as f64
    }

    fn input(alerts: Value, filter: Value) -> Input {
        serde_json::from_value(json!({ "alerts": alerts, "filter": filter, "hexSize": 500, "eps": 150, "minPoints": 2 }))
            .expect("input")
    }

    fn point(id: &str, lon: f64, lat: f64, t: Option<f64>, sub: &str) -> Value {
        json!({ "id": id, "geometry": { "type": "Point", "coordinates": [lon, lat] }, "time": t, "subCategory": sub })
    }

    #[test]
    fn nothing_in_is_nothing_out() {
        let out = run(&input(json!([]), json!({})));
        assert_eq!(
            (out.total, out.kept, out.hexes.len(), out.places.len()),
            (0, 0, 0, 0)
        );
        assert_eq!(out.hour_of_week, vec![vec![0; 24]; 7]);
        assert_eq!(out.busiest, None);
        assert_eq!((out.first, out.last), (None, None));
    }

    #[test]
    fn one_alert_is_one_hexagon_one_hour_and_no_repeat_place() {
        // Monday 2026-10-05 07:00 UTC is 10:00 in Helsinki.
        let out = run(&input(
            json!([point(
                "a",
                24.94,
                60.17,
                Some(ms(2026, 10, 5, 7)),
                "ROAD_WORK"
            )]),
            json!({}),
        ));
        assert_eq!(out.hexes.len(), 1);
        assert_eq!(out.hexes[0].count, 1);
        assert_eq!(out.hour_of_week[0][10], 1);
        assert_eq!(
            out.busiest,
            Some(Busiest {
                weekday: 0,
                hour: 10,
                count: 1
            })
        );
        assert!(out.places.is_empty());
    }

    #[test]
    fn alerts_without_a_place_or_a_time_are_counted_and_kept_out_of_what_needs_them() {
        let alerts = json!([
            { "id": "no-geometry", "time": ms(2026, 10, 5, 7) },
            { "id": "no-time", "geometry": { "type": "Point", "coordinates": [24.94, 60.17] } },
            { "id": "broken", "geometry": { "type": "Point", "coordinates": [] }, "time": null }
        ]);
        let out = run(&input(alerts.clone(), json!({})));
        assert_eq!(
            (out.total, out.kept, out.unlocated, out.untimed),
            (3, 3, 2, 2)
        );
        assert_eq!(out.hexes.iter().map(|h| h.count).sum::<usize>(), 1);
        // A time range drops what carries no time.
        let ranged = run(&input(alerts, json!({ "from": ms(2026, 1, 1, 0) })));
        assert_eq!(ranged.kept, 1);
    }

    #[test]
    fn repeat_places_are_found_and_sorted_by_size() {
        let mut alerts = Vec::new();
        for i in 0..4 {
            alerts.push(point(
                &format!("busy-{i}"),
                24.9400 + f64::from(i) * 0.0002,
                60.17,
                None,
                "ROAD_WORK",
            ));
        }
        for i in 0..2 {
            alerts.push(point(
                &format!("pair-{i}"),
                25.0500 + f64::from(i) * 0.0002,
                60.20,
                None,
                "ROAD_WORK",
            ));
        }
        alerts.push(point("alone", 24.70, 60.30, None, "ROAD_WORK"));
        let out = run(&input(json!(alerts), json!({})));
        assert_eq!(out.places.len(), 2);
        assert_eq!(out.places[0].count, 4);
        assert_eq!(
            out.places[0].ids,
            vec!["busy-0", "busy-1", "busy-2", "busy-3"]
        );
        assert_eq!(out.places[1].count, 2);
        assert!(out.places[0].radius > 0.0 && out.places[0].radius < 150.0);
    }

    #[test]
    fn filters_apply_to_the_whole_set_and_never_hide_their_own_choices() {
        let alerts = json!([
            point("works", 24.94, 60.17, Some(ms(2026, 9, 1, 7)), "ROAD_WORK"),
            point(
                "notice",
                24.95,
                60.17,
                Some(ms(2026, 9, 2, 7)),
                "TRAFFIC_ANNOUNCEMENT"
            ),
            point("old", 24.96, 60.17, Some(ms(2025, 6, 1, 7)), "ROAD_WORK")
        ]);
        let out = run(&input(
            alerts.clone(),
            json!({ "subCategories": ["ROAD_WORK"], "from": ms(2026, 1, 1, 0) }),
        ));
        assert_eq!(out.kept, 1);
        assert_eq!(
            out.sub_categories,
            vec![
                Count {
                    name: "ROAD_WORK".into(),
                    count: 1
                },
                Count {
                    name: "TRAFFIC_ANNOUNCEMENT".into(),
                    count: 1
                }
            ]
        );
        // `to` is exclusive.
        let until = run(&input(alerts.clone(), json!({ "to": ms(2026, 9, 2, 7) })));
        assert_eq!(until.kept, 2);
        // An hour of the week: 2026-09-02 is a Wednesday, 07:00 UTC is 10:00 in summer.
        let hour = run(&input(alerts, json!({ "weekday": 2, "hour": 10 })));
        assert_eq!(hour.kept, 1);
        assert_eq!(
            (hour.first, hour.last),
            (Some(ms(2026, 9, 2, 7)), Some(ms(2026, 9, 2, 7)))
        );
    }

    #[test]
    fn the_busiest_hour_on_a_tie_is_the_earliest_of_the_week() {
        let alerts = json!([
            point("tue", 24.94, 60.17, Some(ms(2026, 10, 6, 7)), "ROAD_WORK"),
            point("mon", 24.94, 60.17, Some(ms(2026, 10, 5, 7)), "ROAD_WORK")
        ]);
        assert_eq!(
            run(&input(alerts, json!({}))).busiest,
            Some(Busiest {
                weekday: 0,
                hour: 10,
                count: 1
            })
        );
    }

    #[test]
    fn the_door_answers_json_and_says_what_was_wrong_with_bad_input() {
        let ok: Value = serde_json::from_str(&analyse(r#"{"alerts": []}"#)).expect("json");
        assert_eq!(ok["total"], 0);
        assert_eq!(ok["hourOfWeek"].as_array().map(Vec::len), Some(7));
        let bad: Value = serde_json::from_str(&analyse("not json")).expect("json");
        assert!(bad["error"]
            .as_str()
            .unwrap_or_default()
            .starts_with("the alerts could not be read"));
        let wrong: Value =
            serde_json::from_str(&analyse(r#"{"alerts": [{"geometry": {}}]}"#)).expect("json");
        assert!(
            wrong["error"].is_string(),
            "an alert without an id is refused: {wrong}"
        );
    }
}
