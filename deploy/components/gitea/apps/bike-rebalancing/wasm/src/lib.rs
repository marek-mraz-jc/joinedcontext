//! The rebalancing planner (T-3328): how far each docking station is from half full, and a route
//! for one service van that takes bikes from the full stations to the empty ones without ever
//! carrying more than it holds. Built natively for `cargo test` and to WebAssembly for the page,
//! where [`plan`] takes and answers JSON.

use serde::{Deserialize, Serialize};
#[cfg(feature = "web")]
use wasm_bindgen::prelude::wasm_bindgen;

/// The mean radius of the Earth, in kilometres.
const EARTH_KM: f64 = 6371.0088;

/// The most stations a route is planned over: the most urgent ones. A van moves bikes at a few
/// dozen stations a shift, and 2-opt over this many stays well under a frame on a phone.
// ponytail: a fixed cap; a time budget when a city has thousands of critical stations at once.
pub const MAX_CANDIDATES: usize = 60;

/// The great-circle distance between two `[lon, lat]` points, in kilometres.
pub fn haversine_km(a: [f64; 2], b: [f64; 2]) -> f64 {
    let (lat1, lat2) = (a[1].to_radians(), b[1].to_radians());
    let dlat = lat2 - lat1;
    let dlon = (b[0] - a[0]).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_KM * h.sqrt().min(1.0).asin()
}

/// A docking station as the page read it; a missing count is `None`, never a guessed zero.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Station {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub lon: Option<f64>,
    pub lat: Option<f64>,
    pub bikes: Option<u32>,
    pub free: Option<u32>,
    pub capacity: Option<u32>,
}

impl Station {
    /// Its position, when it has one on the globe.
    fn at(&self) -> Option<[f64; 2]> {
        match (self.lon, self.lat) {
            (Some(lon), Some(lat))
                if lon.is_finite()
                    && lat.is_finite()
                    && (-180.0..=180.0).contains(&lon)
                    && (-90.0..=90.0).contains(&lat) =>
            {
                Some([lon, lat])
            }
            _ => None,
        }
    }
}

/// What the operator chose; every member has the default the page opens with.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct Settings {
    /// Below this share of its places a station is about to run empty.
    pub low: f64,
    /// Above this share it is about to be full.
    pub high: f64,
    /// The share a station is brought back to.
    pub target: f64,
    /// How many bikes the van carries at most.
    pub van_capacity: u32,
    /// Where the van starts, `[lon, lat]`; at the first station to empty when none.
    pub start: Option<[f64; 2]>,
    /// Stations the operator added to the route though they are not critical.
    pub include: Vec<String>,
    /// Stations the operator took out of the route.
    pub exclude: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            low: 0.15,
            high: 0.85,
            target: 0.5,
            van_capacity: 20,
            start: None,
            include: Vec::new(),
            exclude: Vec::new(),
        }
    }
}

/// How a station stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    /// No bike left.
    Empty,
    /// Below `low`.
    Low,
    Balanced,
    /// Above `high`.
    High,
    /// No free place left.
    Full,
    /// Its counts or its capacity are missing.
    Unknown,
}

impl Level {
    fn critical(self) -> bool {
        matches!(self, Level::Empty | Level::Low | Level::High | Level::Full)
    }
}

/// One station's standing.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Need {
    pub id: String,
    pub level: Level,
    /// Bikes over its places, 0 to 1; `None` when unknown.
    pub fill: Option<f64>,
    /// Bikes above the target: positive to take away, negative to bring.
    pub surplus: i64,
}

/// How `station` stands under `settings`.
pub fn assess(station: &Station, settings: &Settings) -> Need {
    let capacity =
        station
            .capacity
            .filter(|c| *c > 0)
            .or_else(|| match (station.bikes, station.free) {
                (Some(bikes), Some(free)) if bikes + free > 0 => Some(bikes + free),
                _ => None,
            });
    let (Some(capacity), Some(bikes)) = (capacity, station.bikes) else {
        return Need {
            id: station.id.clone(),
            level: Level::Unknown,
            fill: None,
            surplus: 0,
        };
    };
    let fill = (f64::from(bikes) / f64::from(capacity)).min(1.0);
    let level = if bikes == 0 {
        Level::Empty
    } else if station.free == Some(0) || bikes >= capacity {
        Level::Full
    } else if fill < settings.low {
        Level::Low
    } else if fill > settings.high {
        Level::High
    } else {
        Level::Balanced
    };
    let target = (f64::from(capacity) * settings.target.clamp(0.0, 1.0)).round() as i64;
    Need {
        id: station.id.clone(),
        level,
        fill: Some(fill),
        surplus: i64::from(bikes.min(capacity)) - target,
    }
}

/// Take bikes away or bring them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    Pick,
    Drop,
}

/// One stop of the van.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stop {
    pub id: String,
    pub name: String,
    pub at: [f64; 2],
    pub action: Action,
    pub bikes: u32,
    /// Bikes in the van after the stop.
    pub load: u32,
    /// From the previous stop, or from the start for the first.
    pub leg_km: f64,
}

/// The van's route.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    pub stops: Vec<Stop>,
    pub km: f64,
    /// Bikes taken away, which is also the bikes brought.
    pub moved: u32,
}

/// A station the route may visit: where it is and what it needs.
#[derive(Debug, Clone)]
struct Candidate {
    id: String,
    name: String,
    at: [f64; 2],
    surplus: i64,
}

/// The stops a visit order yields: at each station the van takes what it can carry or brings what
/// it holds, and a stop that moves nothing is left out.
fn simulate(order: &[usize], all: &[Candidate], capacity: u32, start: Option<[f64; 2]>) -> Route {
    let mut stops = Vec::new();
    let mut load: u32 = 0;
    let mut moved: u32 = 0;
    let mut km = 0.0;
    let mut previous = start;
    for &index in order {
        let candidate = &all[index];
        let (action, bikes) = if candidate.surplus > 0 {
            let room = capacity - load;
            (
                Action::Pick,
                u32::try_from(candidate.surplus)
                    .unwrap_or(u32::MAX)
                    .min(room),
            )
        } else {
            let short = u32::try_from(-candidate.surplus).unwrap_or(u32::MAX);
            (Action::Drop, short.min(load))
        };
        if bikes == 0 {
            continue;
        }
        match action {
            Action::Pick => load += bikes,
            Action::Drop => {
                load -= bikes;
                moved += bikes;
            }
        }
        let leg_km = previous.map_or(0.0, |from| haversine_km(from, candidate.at));
        km += leg_km;
        previous = Some(candidate.at);
        stops.push(Stop {
            id: candidate.id.clone(),
            name: candidate.name.clone(),
            at: candidate.at,
            action,
            bikes,
            load,
            leg_km,
        });
    }
    // Bikes still in the van at the end were taken for nothing: give the route no credit for them.
    Route { stops, km, moved }
}

/// Better means more bikes brought where they are missing, then more stations served (the bikes
/// spread over every station that lacks them rather than filling one), then fewer kilometres.
fn better(a: &Route, b: &Route) -> bool {
    (a.moved, a.stops.len()) > (b.moved, b.stops.len())
        || ((a.moved, a.stops.len()) == (b.moved, b.stops.len()) && a.km + 1e-9 < b.km)
}

/// The van's route over `candidates`: nearest neighbour under the van's capacity, then 2-opt
/// while it brings more bikes or drives less.
fn route(all: &[Candidate], capacity: u32, start: Option<[f64; 2]>) -> Route {
    if all.is_empty() || capacity == 0 {
        return Route {
            stops: Vec::new(),
            km: 0.0,
            moved: 0,
        };
    }
    let mut left: Vec<usize> = (0..all.len()).collect();
    let mut order = Vec::with_capacity(all.len());
    let mut load: u32 = 0;
    // With no start, the van begins at the station with the most bikes to take.
    let mut here = start.unwrap_or_else(|| {
        all.iter()
            .max_by(|a, b| a.surplus.cmp(&b.surplus).then_with(|| b.id.cmp(&a.id)))
            .map_or([0.0, 0.0], |c| c.at)
    });
    loop {
        let next = left
            .iter()
            .enumerate()
            .filter(|(_, &i)| {
                let c = &all[i];
                (c.surplus > 0 && load < capacity) || (c.surplus < 0 && load > 0)
            })
            .min_by(|(_, &a), (_, &b)| {
                haversine_km(here, all[a].at)
                    .total_cmp(&haversine_km(here, all[b].at))
                    .then_with(|| all[a].id.cmp(&all[b].id))
            })
            .map(|(position, &i)| (position, i));
        let Some((position, index)) = next else {
            break;
        };
        left.remove(position);
        let c = &all[index];
        load = if c.surplus > 0 {
            load + u32::try_from(c.surplus)
                .unwrap_or(u32::MAX)
                .min(capacity - load)
        } else {
            load - u32::try_from(-c.surplus).unwrap_or(u32::MAX).min(load)
        };
        here = c.at;
        order.push(index);
    }
    let mut best = simulate(&order, all, capacity, start);
    let mut improved = true;
    while improved {
        improved = false;
        for i in 0..order.len() {
            for j in i + 1..order.len() {
                order[i..=j].reverse();
                let tried = simulate(&order, all, capacity, start);
                if better(&tried, &best) {
                    best = tried;
                    improved = true;
                } else {
                    order[i..=j].reverse();
                }
            }
        }
    }
    best
}

/// What the page sends.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Input {
    pub stations: Vec<Station>,
    #[serde(default)]
    pub settings: Settings,
}

/// What the page gets back.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Output {
    pub needs: Vec<Need>,
    pub route: Route,
}

/// Every station's standing and the van's route.
pub fn run(input: &Input) -> Output {
    let settings = &input.settings;
    let needs: Vec<Need> = input.stations.iter().map(|s| assess(s, settings)).collect();
    let mut candidates: Vec<Candidate> = input
        .stations
        .iter()
        .zip(&needs)
        .filter(|(s, need)| {
            !settings.exclude.contains(&s.id)
                && need.surplus != 0
                && (need.level.critical() || settings.include.contains(&s.id))
        })
        .filter_map(|(s, need)| {
            Some(Candidate {
                id: s.id.clone(),
                name: s.name.clone(),
                at: s.at()?,
                surplus: need.surplus,
            })
        })
        .collect();
    // The most urgent first, so the cap keeps them.
    candidates.sort_by(|a, b| {
        b.surplus
            .abs()
            .cmp(&a.surplus.abs())
            .then_with(|| a.id.cmp(&b.id))
    });
    candidates.truncate(MAX_CANDIDATES);
    let start = settings.start.filter(|p| {
        p.iter().all(|v| v.is_finite())
            && (-180.0..=180.0).contains(&p[0])
            && (-90.0..=90.0).contains(&p[1])
    });
    Output {
        needs,
        route: route(&candidates, settings.van_capacity, start),
    }
}

/// The page's entry: [`Input`] as JSON in, [`Output`] as JSON out, or `{"error": "…"}` naming
/// what could not be read.
#[cfg_attr(feature = "web", wasm_bindgen)]
pub fn plan(input: &str) -> String {
    match serde_json::from_str::<Input>(input) {
        Ok(input) => serde_json::to_string(&run(&input))
            .unwrap_or_else(|_| r#"{"error":"the plan could not be written"}"#.to_owned()),
        Err(err) => {
            serde_json::json!({ "error": format!("the stations could not be read: {err}") })
                .to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn station(id: &str, lon: f64, lat: f64, bikes: u32, capacity: u32) -> Station {
        Station {
            id: id.into(),
            name: id.into(),
            lon: Some(lon),
            lat: Some(lat),
            bikes: Some(bikes),
            free: Some(capacity - bikes),
            capacity: Some(capacity),
        }
    }

    fn input(stations: Vec<Station>, settings: Settings) -> Input {
        Input { stations, settings }
    }

    #[test]
    fn the_distance_is_the_great_circle_one() {
        // Kaisaniemi to the Helsinki-Vantaa airport weather station: about 17.2 km.
        let d = haversine_km([24.9445896, 60.1752281], [24.9727402, 60.3293686]);
        assert!((d - 17.2).abs() < 0.1, "{d}");
        assert_eq!(haversine_km([24.9, 60.1], [24.9, 60.1]), 0.0);
        // Across the antimeridian and between the poles, no NaN.
        assert!((haversine_km([180.0, 0.0], [-180.0, 0.0])).abs() < 1e-6);
        let pole = haversine_km([0.0, 90.0], [0.0, -90.0]);
        assert!((pole - std::f64::consts::PI * EARTH_KM).abs() < 1e-6);
    }

    #[test]
    fn a_station_stands_by_its_fill() {
        let s = Settings::default();
        assert_eq!(
            assess(&station("a", 0.0, 0.0, 0, 20), &s).level,
            Level::Empty
        );
        assert_eq!(assess(&station("a", 0.0, 0.0, 2, 20), &s).level, Level::Low);
        assert_eq!(
            assess(&station("a", 0.0, 0.0, 10, 20), &s).level,
            Level::Balanced
        );
        assert_eq!(
            assess(&station("a", 0.0, 0.0, 18, 20), &s).level,
            Level::High
        );
        let full = assess(&station("a", 0.0, 0.0, 20, 20), &s);
        assert_eq!((full.level, full.surplus), (Level::Full, 10));
        assert_eq!(assess(&station("a", 0.0, 0.0, 2, 20), &s).surplus, -8);
    }

    #[test]
    fn missing_counts_are_unknown_never_zero() {
        let s = Settings::default();
        let mut no_capacity = station("a", 0.0, 0.0, 3, 10);
        no_capacity.capacity = None;
        // Bikes and free places still give the capacity.
        assert_eq!(assess(&no_capacity, &s).fill, Some(0.3));
        no_capacity.free = None;
        assert_eq!(assess(&no_capacity, &s).level, Level::Unknown);
        let mut no_bikes = station("b", 0.0, 0.0, 3, 10);
        no_bikes.bikes = None;
        let need = assess(&no_bikes, &s);
        assert_eq!(
            (need.level, need.fill, need.surplus),
            (Level::Unknown, None, 0)
        );
        let mut zero = station("c", 0.0, 0.0, 0, 1);
        zero.capacity = Some(0);
        zero.free = Some(0);
        assert_eq!(assess(&zero, &s).level, Level::Unknown);
    }

    #[test]
    fn no_station_and_a_single_one_plan_nothing() {
        let out = run(&input(Vec::new(), Settings::default()));
        assert!(out.needs.is_empty() && out.route.stops.is_empty());
        let out = run(&input(
            vec![station("a", 24.9, 60.1, 20, 20)],
            Settings::default(),
        ));
        assert_eq!(out.needs.len(), 1);
        // Bikes to take but nowhere to bring them: nothing is moved.
        assert_eq!(out.route.moved, 0);
        assert!(out.route.stops.iter().all(|s| s.action == Action::Pick));
    }

    #[test]
    fn the_van_takes_from_the_full_and_brings_to_the_empty_within_its_capacity() {
        let stations = vec![
            station("full", 24.90, 60.17, 30, 30),
            station("empty", 24.95, 60.17, 0, 30),
            station("ok", 24.92, 60.17, 15, 30),
        ];
        let settings = Settings {
            van_capacity: 10,
            ..Settings::default()
        };
        let out = run(&input(stations, settings));
        let stops: Vec<(&str, Action, u32, u32)> = out
            .route
            .stops
            .iter()
            .map(|s| (s.id.as_str(), s.action, s.bikes, s.load))
            .collect();
        assert_eq!(
            stops,
            [
                ("full", Action::Pick, 10, 10),
                ("empty", Action::Drop, 10, 0)
            ]
        );
        assert_eq!(out.route.moved, 10);
        assert!(out.route.stops.iter().all(|s| s.load <= 10));
        assert!((out.route.km - haversine_km([24.90, 60.17], [24.95, 60.17])).abs() < 1e-9);
    }

    #[test]
    fn excluded_and_included_stations_are_the_operators_choice() {
        let stations = vec![
            station("full", 24.90, 60.17, 30, 30),
            station("empty", 24.95, 60.17, 0, 30),
            station("lowish", 24.93, 60.17, 10, 30),
        ];
        let out = run(&input(
            stations.clone(),
            Settings {
                exclude: vec!["empty".into()],
                ..Settings::default()
            },
        ));
        assert!(out.route.stops.iter().all(|s| s.id != "empty"));
        assert_eq!(out.route.moved, 0);
        // A balanced station brought in is served too: 10 of 30 is short of 15.
        let out = run(&input(
            stations,
            Settings {
                include: vec!["lowish".into()],
                van_capacity: 30,
                ..Settings::default()
            },
        ));
        let lowish = out
            .route
            .stops
            .iter()
            .find(|s| s.id == "lowish")
            .expect("in the route");
        assert_eq!((lowish.action, lowish.bikes), (Action::Drop, 5));
    }

    #[test]
    fn a_station_off_the_globe_is_never_routed() {
        let mut lost = station("lost", 24.95, 60.17, 0, 30);
        lost.lat = Some(91.0);
        let mut nan = station("nan", 24.95, 60.17, 0, 30);
        nan.lon = Some(f64::NAN);
        let edge = station("edge", 180.0, -90.0, 0, 30);
        let out = run(&input(
            vec![station("full", 179.9, -89.9, 30, 30), lost, nan, edge],
            Settings::default(),
        ));
        let ids: Vec<&str> = out.route.stops.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["full", "edge"]);
        assert_eq!(out.needs.len(), 4, "every station is still assessed");
    }

    #[test]
    fn two_opt_untangles_a_crossing_route() {
        // Four pick-and-drop pairs around a square; the start is in a corner.
        let stations = vec![
            station("p1", 24.90, 60.10, 30, 30),
            station("d1", 24.90, 60.20, 0, 30),
            station("p2", 25.00, 60.20, 30, 30),
            station("d2", 25.00, 60.10, 0, 30),
        ];
        let out = run(&input(
            stations,
            Settings {
                van_capacity: 100,
                start: Some([24.89, 60.09]),
                ..Settings::default()
            },
        ));
        assert_eq!(out.route.moved, 30);
        // The square's perimeter, not its diagonals.
        let perimeter = haversine_km([24.89, 60.09], [24.90, 60.10])
            + haversine_km([24.90, 60.10], [24.90, 60.20])
            + haversine_km([24.90, 60.20], [25.00, 60.20])
            + haversine_km([25.00, 60.20], [25.00, 60.10]);
        assert!(
            out.route.km <= perimeter + 1e-9,
            "{} > {perimeter}",
            out.route.km
        );
        let mut load = 0i64;
        for stop in &out.route.stops {
            load += if stop.action == Action::Pick { 1 } else { -1 } * i64::from(stop.bikes);
            assert!((0..=100).contains(&load));
            assert_eq!(load, i64::from(stop.load));
        }
    }

    #[test]
    fn a_van_of_no_capacity_plans_no_route() {
        let out = run(&input(
            vec![
                station("full", 24.90, 60.17, 30, 30),
                station("empty", 24.95, 60.17, 0, 30),
            ],
            Settings {
                van_capacity: 0,
                ..Settings::default()
            },
        ));
        assert!(out.route.stops.is_empty());
    }

    #[test]
    fn the_candidates_are_capped_at_the_most_urgent() {
        let stations: Vec<Station> = (0..200)
            .map(|i| {
                let bikes = if i % 2 == 0 { 30 } else { 0 };
                station(
                    &format!("s{i:03}"),
                    24.9 + f64::from(i) * 0.001,
                    60.17,
                    bikes,
                    30,
                )
            })
            .collect();
        let out = run(&input(
            stations,
            Settings {
                van_capacity: 1000,
                ..Settings::default()
            },
        ));
        assert!(out.route.stops.len() <= MAX_CANDIDATES);
        assert_eq!(out.needs.len(), 200);
    }

    #[test]
    fn the_entry_answers_json_and_names_what_it_could_not_read() {
        let answer: serde_json::Value = serde_json::from_str(&plan(
            r#"{"stations":[{"id":"a","name":"A","lon":24.9,"lat":60.1,"bikes":0,"free":10,"capacity":10}]}"#,
        ))
        .expect("json");
        assert_eq!(answer["needs"][0]["level"], "empty");
        assert_eq!(answer["route"]["stops"], serde_json::json!([]));
        let refused: serde_json::Value = serde_json::from_str(&plan("not json")).expect("json");
        assert!(refused["error"]
            .as_str()
            .unwrap_or_default()
            .starts_with("the stations could not be read"));
        let unknown: serde_json::Value =
            serde_json::from_str(&plan(r#"{"stations":[],"extra":1}"#)).expect("json");
        assert!(unknown["error"].is_string());
    }
}
