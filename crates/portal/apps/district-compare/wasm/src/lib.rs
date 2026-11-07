//! Helsinki district comparison (T-3335): point-in-polygon assignment of events, bikes, alerts
//! and air quality observations across city districts, area calculation, per km² densities and
//! competition ranking computed in the browser.

pub mod geo;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(feature = "browser")]
use wasm_bindgen::prelude::wasm_bindgen;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DistrictInput {
    pub code: String,
    pub name: String,
    #[serde(default)]
    pub geometry: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BikeInput {
    #[serde(default)]
    pub at: Option<Vec<f64>>,
    #[serde(default)]
    pub slots: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AirInput {
    #[serde(default)]
    pub at: Option<Vec<f64>>,
    #[serde(default)]
    pub pm25: Option<f64>,
    #[serde(default)]
    pub aqi: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    #[serde(default)]
    pub districts: Vec<DistrictInput>,
    #[serde(default)]
    pub events: Vec<Vec<f64>>,
    #[serde(default)]
    pub bikes: Vec<BikeInput>,
    #[serde(default)]
    pub alerts: Vec<Vec<f64>>,
    #[serde(default)]
    pub air: Vec<AirInput>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PerKm2 {
    pub events: Option<f64>,
    pub bikes: Option<f64>,
    pub bike_slots: Option<f64>,
    pub alerts: Option<f64>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DistrictOutput {
    pub code: String,
    pub name: String,
    pub area_km2: f64,
    pub events: usize,
    pub bikes: usize,
    pub bike_slots: usize,
    pub alerts: usize,
    pub pm25: Option<f64>,
    pub aqi: Option<f64>,
    pub per_km2: PerKm2,
    pub rank: BTreeMap<String, Option<usize>>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OutsideOutput {
    pub events: usize,
    pub bikes: usize,
    pub alerts: usize,
    pub air: usize,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Output {
    pub districts: Vec<DistrictOutput>,
    pub outside: OutsideOutput,
}

struct PreparedDistrict {
    code: String,
    name: String,
    area_km2: f64,
    geometry: Value,
    bbox: Option<[f64; 4]>,
    has_valid_geometry: bool,
}

impl PreparedDistrict {
    fn new(d: DistrictInput) -> Self {
        let bbox = geo::bbox(&d.geometry);
        let area = geo::area_km2(&d.geometry);
        let has_valid_geometry = bbox.is_some() && area > 0.0;
        Self {
            code: d.code,
            name: d.name,
            area_km2: if has_valid_geometry { area } else { 0.0 },
            geometry: d.geometry,
            bbox,
            has_valid_geometry,
        }
    }

    fn contains(&self, point: (f64, f64)) -> bool {
        if !self.has_valid_geometry {
            return false;
        }
        if let Some(b) = &self.bbox {
            if !geo::bbox_contains(b, point) {
                return false;
            }
        }
        geo::point_in_geometry(point, &self.geometry)
    }
}

#[derive(Default)]
struct DistrictCounts {
    events: usize,
    bikes: usize,
    bike_slots: usize,
    alerts: usize,
    pm25_vals: Vec<f64>,
    aqi_vals: Vec<f64>,
}

fn valid_coordinate(coords: &[f64]) -> Option<(f64, f64)> {
    if coords.len() < 2 {
        return None;
    }
    let lon = coords[0];
    let lat = coords[1];
    if lon.is_finite()
        && lat.is_finite()
        && (-180.0..=180.0).contains(&lon)
        && (-90.0..=90.0).contains(&lat)
    {
        Some((lon, lat))
    } else {
        None
    }
}

/// Competition ranking (1224 ranking): rank 1 is highest value, ties share the rank,
/// a null value ranks last and its rank is None.
pub fn competition_rank(values: &[Option<f64>]) -> Vec<Option<usize>> {
    let mut indexed: Vec<(usize, f64)> = values
        .iter()
        .enumerate()
        .filter_map(|(i, &v)| v.map(|val| (i, val)))
        .collect();

    indexed.sort_by(|a, b| b.1.total_cmp(&a.1));

    let mut ranks = vec![None; values.len()];
    let mut current_rank = 1;
    for (pos, &(idx, val)) in indexed.iter().enumerate() {
        if pos > 0 {
            let prev_val = indexed[pos - 1].1;
            if (val - prev_val).abs() > 1e-9 {
                current_rank = pos + 1;
            }
        }
        ranks[idx] = Some(current_rank);
    }
    ranks
}

/// Runs the complete comparison analysis over the supplied input.
pub fn run(input: &Input) -> Output {
    let prepared: Vec<PreparedDistrict> = input
        .districts
        .iter()
        .map(|d| {
            PreparedDistrict::new(DistrictInput {
                code: d.code.clone(),
                name: d.name.clone(),
                geometry: d.geometry.clone(),
            })
        })
        .collect();

    let mut counts: Vec<DistrictCounts> = (0..prepared.len())
        .map(|_| DistrictCounts::default())
        .collect();
    let mut outside = OutsideOutput {
        events: 0,
        bikes: 0,
        alerts: 0,
        air: 0,
    };

    // 1. Events
    for ev in &input.events {
        if let Some(pt) = valid_coordinate(ev) {
            let mut matched = false;
            for (idx, d) in prepared.iter().enumerate() {
                if d.contains(pt) {
                    counts[idx].events += 1;
                    matched = true;
                }
            }
            if !matched {
                outside.events += 1;
            }
        } else {
            outside.events += 1;
        }
    }

    // 2. Bikes
    for bike in &input.bikes {
        let pt_opt = bike.at.as_deref().and_then(valid_coordinate);
        if let Some(pt) = pt_opt {
            let mut matched = false;
            let slots = bike.slots.unwrap_or(0).max(0) as usize;
            for (idx, d) in prepared.iter().enumerate() {
                if d.contains(pt) {
                    counts[idx].bikes += 1;
                    counts[idx].bike_slots += slots;
                    matched = true;
                }
            }
            if !matched {
                outside.bikes += 1;
            }
        } else {
            outside.bikes += 1;
        }
    }

    // 3. Alerts
    for al in &input.alerts {
        if let Some(pt) = valid_coordinate(al) {
            let mut matched = false;
            for (idx, d) in prepared.iter().enumerate() {
                if d.contains(pt) {
                    counts[idx].alerts += 1;
                    matched = true;
                }
            }
            if !matched {
                outside.alerts += 1;
            }
        } else {
            outside.alerts += 1;
        }
    }

    // 4. Air Quality
    for station in &input.air {
        let pt_opt = station.at.as_deref().and_then(valid_coordinate);
        if let Some(pt) = pt_opt {
            let mut matched = false;
            for (idx, d) in prepared.iter().enumerate() {
                if d.contains(pt) {
                    if let Some(p) = station.pm25 {
                        if p.is_finite() {
                            counts[idx].pm25_vals.push(p);
                        }
                    }
                    if let Some(a) = station.aqi {
                        if a.is_finite() {
                            counts[idx].aqi_vals.push(a);
                        }
                    }
                    matched = true;
                }
            }
            if !matched {
                outside.air += 1;
            }
        } else {
            outside.air += 1;
        }
    }

    // Prepare measures for ranking
    let n = prepared.len();
    let mut ev_vals = Vec::with_capacity(n);
    let mut bk_vals = Vec::with_capacity(n);
    let mut bs_vals = Vec::with_capacity(n);
    let mut al_vals = Vec::with_capacity(n);
    let mut pm_vals = Vec::with_capacity(n);
    let mut aq_vals = Vec::with_capacity(n);
    let mut ev_per_km2 = Vec::with_capacity(n);
    let mut bk_per_km2 = Vec::with_capacity(n);
    let mut bs_per_km2 = Vec::with_capacity(n);
    let mut al_per_km2 = Vec::with_capacity(n);

    for (idx, d) in prepared.iter().enumerate() {
        let c = &counts[idx];
        ev_vals.push(Some(c.events as f64));
        bk_vals.push(Some(c.bikes as f64));
        bs_vals.push(Some(c.bike_slots as f64));
        al_vals.push(Some(c.alerts as f64));

        let pm = if c.pm25_vals.is_empty() {
            None
        } else {
            Some(c.pm25_vals.iter().sum::<f64>() / c.pm25_vals.len() as f64)
        };
        pm_vals.push(pm);

        let aq = if c.aqi_vals.is_empty() {
            None
        } else {
            Some(c.aqi_vals.iter().sum::<f64>() / c.aqi_vals.len() as f64)
        };
        aq_vals.push(aq);

        if d.area_km2 > 0.0 {
            ev_per_km2.push(Some(c.events as f64 / d.area_km2));
            bk_per_km2.push(Some(c.bikes as f64 / d.area_km2));
            bs_per_km2.push(Some(c.bike_slots as f64 / d.area_km2));
            al_per_km2.push(Some(c.alerts as f64 / d.area_km2));
        } else {
            ev_per_km2.push(None);
            bk_per_km2.push(None);
            bs_per_km2.push(None);
            al_per_km2.push(None);
        }
    }

    let r_events = competition_rank(&ev_vals);
    let r_bikes = competition_rank(&bk_vals);
    let r_bike_slots = competition_rank(&bs_vals);
    let r_alerts = competition_rank(&al_vals);
    let r_pm25 = competition_rank(&pm_vals);
    let r_aqi = competition_rank(&aq_vals);
    let r_ev_pk = competition_rank(&ev_per_km2);
    let r_bk_pk = competition_rank(&bk_per_km2);
    let r_bs_pk = competition_rank(&bs_per_km2);
    let r_al_pk = competition_rank(&al_per_km2);

    let districts = prepared
        .into_iter()
        .enumerate()
        .map(|(idx, d)| {
            let c = &counts[idx];
            let mut rank = BTreeMap::new();
            rank.insert("events".to_string(), r_events[idx]);
            rank.insert("bikes".to_string(), r_bikes[idx]);
            rank.insert("bikeSlots".to_string(), r_bike_slots[idx]);
            rank.insert("alerts".to_string(), r_alerts[idx]);
            rank.insert("pm25".to_string(), r_pm25[idx]);
            rank.insert("aqi".to_string(), r_aqi[idx]);
            rank.insert("eventsPerKm2".to_string(), r_ev_pk[idx]);
            rank.insert("bikesPerKm2".to_string(), r_bk_pk[idx]);
            rank.insert("bikeSlotsPerKm2".to_string(), r_bs_pk[idx]);
            rank.insert("alertsPerKm2".to_string(), r_al_pk[idx]);

            DistrictOutput {
                code: d.code,
                name: d.name,
                area_km2: d.area_km2,
                events: c.events,
                bikes: c.bikes,
                bike_slots: c.bike_slots,
                alerts: c.alerts,
                pm25: pm_vals[idx],
                aqi: aq_vals[idx],
                per_km2: PerKm2 {
                    events: ev_per_km2[idx],
                    bikes: bk_per_km2[idx],
                    bike_slots: bs_per_km2[idx],
                    alerts: al_per_km2[idx],
                },
                rank,
            }
        })
        .collect();

    Output { districts, outside }
}

/// WASM entry point: reads JSON input and serialises JSON output or an error object.
#[cfg_attr(feature = "browser", wasm_bindgen)]
pub fn compare(input: &str) -> String {
    match serde_json::from_str::<Input>(input) {
        Ok(parsed) => serde_json::to_string(&run(&parsed))
            .unwrap_or_else(|e| serde_json::json!({ "error": e.to_string() }).to_string()),
        Err(e) => serde_json::json!({ "error": format!("the districts could not be read: {e}") })
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_district(
        code: &str,
        name: &str,
        min_lon: f64,
        min_lat: f64,
        max_lon: f64,
        max_lat: f64,
    ) -> DistrictInput {
        DistrictInput {
            code: code.to_string(),
            name: name.to_string(),
            geometry: json!({
                "type": "Polygon",
                "coordinates": [
                    [
                        [min_lon, min_lat],
                        [max_lon, min_lat],
                        [max_lon, max_lat],
                        [min_lon, max_lat],
                        [min_lon, min_lat]
                    ]
                ]
            }),
        }
    }

    #[test]
    fn empty_input() {
        let input = Input {
            districts: vec![],
            events: vec![],
            bikes: vec![],
            alerts: vec![],
            air: vec![],
        };
        let out = run(&input);
        assert_eq!(out.districts.len(), 0);
        assert_eq!(
            out.outside,
            OutsideOutput {
                events: 0,
                bikes: 0,
                alerts: 0,
                air: 0
            }
        );
    }

    #[test]
    fn district_with_no_geometry() {
        let input = Input {
            districts: vec![DistrictInput {
                code: "broken".to_string(),
                name: "Broken".to_string(),
                geometry: Value::Null,
            }],
            events: vec![vec![24.94, 60.17]],
            bikes: vec![],
            alerts: vec![],
            air: vec![],
        };
        let out = run(&input);
        assert_eq!(out.districts.len(), 1);
        let d = &out.districts[0];
        assert_eq!(d.area_km2, 0.0);
        assert_eq!(d.events, 0);
        assert_eq!(d.per_km2.events, None);
        assert_eq!(d.rank.get("events"), Some(&Some(1)));
        assert_eq!(d.rank.get("eventsPerKm2"), Some(&None));
        assert_eq!(out.outside.events, 1);
    }

    #[test]
    fn ties_in_ranks() {
        let d1 = sample_district("d1", "District 1", 24.0, 60.0, 24.1, 60.1);
        let d2 = sample_district("d2", "District 2", 24.2, 60.0, 24.3, 60.1);
        let d3 = sample_district("d3", "District 3", 24.4, 60.0, 24.5, 60.1);

        let input = Input {
            districts: vec![d1, d2, d3],
            events: vec![
                vec![24.05, 60.05],
                vec![24.05, 60.06],
                vec![24.25, 60.05],
                vec![24.25, 60.06],
                vec![24.45, 60.05],
            ],
            bikes: vec![],
            alerts: vec![],
            air: vec![],
        };
        let out = run(&input);
        assert_eq!(out.districts[0].events, 2);
        assert_eq!(out.districts[1].events, 2);
        assert_eq!(out.districts[2].events, 1);

        // Ties share rank 1; next is rank 3
        assert_eq!(out.districts[0].rank.get("events"), Some(&Some(1)));
        assert_eq!(out.districts[1].rank.get("events"), Some(&Some(1)));
        assert_eq!(out.districts[2].rank.get("events"), Some(&Some(3)));
    }

    #[test]
    fn null_pm25() {
        let d1 = sample_district("d1", "District 1", 24.0, 60.0, 24.1, 60.1);
        let d2 = sample_district("d2", "District 2", 24.2, 60.0, 24.3, 60.1);

        let input = Input {
            districts: vec![d1, d2],
            events: vec![],
            bikes: vec![],
            alerts: vec![],
            air: vec![AirInput {
                at: Some(vec![24.05, 60.05]),
                pm25: Some(15.0),
                aqi: Some(2.0),
            }],
        };
        let out = run(&input);
        assert_eq!(out.districts[0].pm25, Some(15.0));
        assert_eq!(out.districts[0].rank.get("pm25"), Some(&Some(1)));

        // d2 has no station, pm25 is None and rank is None
        assert_eq!(out.districts[1].pm25, None);
        assert_eq!(out.districts[1].rank.get("pm25"), Some(&None));
    }

    #[test]
    fn bad_json() {
        let answer = compare("this is not json");
        let val: Value = serde_json::from_str(&answer).expect("error response is json");
        assert!(val["error"]
            .as_str()
            .unwrap()
            .starts_with("the districts could not be read"));
    }

    #[test]
    fn point_in_two_overlapping_districts() {
        // Two districts overlapping on [24.0, 60.0] x [24.2, 60.2]
        let d1 = sample_district("d1", "District 1", 24.0, 60.0, 24.2, 60.2);
        let d2 = sample_district("d2", "District 2", 24.1, 60.1, 24.3, 60.3);

        let input = Input {
            districts: vec![d1, d2],
            events: vec![vec![24.15, 60.15]],
            bikes: vec![],
            alerts: vec![],
            air: vec![],
        };
        let out = run(&input);
        assert_eq!(out.districts[0].events, 1);
        assert_eq!(out.districts[1].events, 1);
        assert_eq!(out.outside.events, 0);
    }

    #[test]
    fn coordinates_outside_bounds() {
        let d1 = sample_district("d1", "District 1", 24.0, 60.0, 24.2, 60.2);
        let input = Input {
            districts: vec![d1],
            events: vec![
                vec![200.0, 60.0],    // invalid lon
                vec![24.1, 95.0],     // invalid lat
                vec![f64::NAN, 60.0], // NaN
            ],
            bikes: vec![BikeInput {
                at: Some(vec![300.0, 60.0]),
                slots: Some(10),
            }],
            alerts: vec![vec![-250.0, 0.0]],
            air: vec![AirInput {
                at: Some(vec![24.1, -100.0]),
                pm25: Some(10.0),
                aqi: None,
            }],
        };
        let out = run(&input);
        assert_eq!(out.districts[0].events, 0);
        assert_eq!(out.outside.events, 3);
        assert_eq!(out.outside.bikes, 1);
        assert_eq!(out.outside.alerts, 1);
        assert_eq!(out.outside.air, 1);
    }

    #[test]
    fn bikes_and_alerts_count_where_they_stand_and_outside_where_no_district_holds_them() {
        let input = Input {
            districts: vec![sample_district("1", "Kamppi", 24.0, 60.0, 25.0, 61.0)],
            events: vec![],
            bikes: vec![
                BikeInput {
                    at: Some(vec![24.5, 60.5]),
                    slots: Some(12),
                },
                BikeInput {
                    at: Some(vec![26.0, 60.5]),
                    slots: Some(8),
                },
                BikeInput {
                    at: None,
                    slots: Some(4),
                },
                // A negative slot count counts as none.
                BikeInput {
                    at: Some(vec![24.6, 60.6]),
                    slots: Some(-3),
                },
            ],
            alerts: vec![vec![24.5, 60.5], vec![30.0, 60.0], vec![]],
            air: vec![AirInput {
                at: Some(vec![30.0, 60.0]),
                pm25: Some(5.0),
                aqi: None,
            }],
        };
        let out = run(&input);
        assert_eq!(out.districts[0].bikes, 2);
        assert_eq!(out.districts[0].bike_slots, 12);
        assert_eq!(out.districts[0].alerts, 1);
        assert_eq!(out.outside.bikes, 2);
        assert_eq!(out.outside.alerts, 2);
        assert_eq!(out.outside.air, 1);
        assert_eq!(out.districts[0].pm25, None);
    }

    #[test]
    fn the_entry_answers_a_whole_comparison_as_json() {
        let raw = compare(
            &json!({
                "districts": [{ "code": "1", "name": "Kamppi", "geometry": { "type": "Polygon", "coordinates": [[[24.0, 60.0], [25.0, 60.0], [25.0, 61.0], [24.0, 61.0], [24.0, 60.0]]] } }],
                "events": [[24.5, 60.5]]
            })
            .to_string(),
        );
        let parsed: Value = serde_json::from_str(&raw).expect("json");
        assert_eq!(parsed["districts"][0]["events"], 1);
        assert!(parsed.get("error").is_none());
    }
}
