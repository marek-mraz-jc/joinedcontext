//! Stops and rides from HSL's own registers (T-3356): every stop and platform of the region, and
//! every line variant with its stops in order. The registers carry no timetable, so a ride between
//! two stops of a line takes the distance along the street (1.2 times the straight line) at the
//! mode's usual speed with its stops included.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde::Deserialize;

use crate::geo::to_metres;
use crate::stops::{Ride, Stop};

/// A line is this much longer than the straight line between its stops.
pub const ROUTE_DETOUR: f64 = 1.2;

/// Kilometres an hour, stops included, as HSL's own journey planner times them on average.
pub fn speed_kmh(mode: &str) -> f64 {
    match mode {
        "metro" => 40.0,
        "train" => 50.0,
        "tram" => 15.0,
        "ferry" => 18.0,
        _ => 20.0,
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetStop {
    pub id: String,
    pub lon: f64,
    pub lat: f64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetRoute {
    /// The line number riders see.
    pub name: String,
    #[serde(default)]
    pub mode: Option<String>,
    /// Its stops in order, by the ids of `NetStop`.
    pub stops: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Network {
    #[serde(default)]
    pub stops: Vec<NetStop>,
    #[serde(default)]
    pub routes: Vec<NetRoute>,
}

/// A stop's name and sign code beside the `Stop` the router uses.
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub name: Option<String>,
    pub code: Option<String>,
}

/// The stops with a place, their labels, and the rides between consecutive stops of every line.
/// A stop a line names that the register does not hold breaks the line there; the parts on either
/// side still ride.
pub fn build(network: &Network) -> (Vec<Stop>, Vec<Label>, Vec<Ride>) {
    let mut index: HashMap<&str, usize> = HashMap::new();
    let mut stops = Vec::new();
    let mut labels = Vec::new();
    for stop in &network.stops {
        let placed = stop.lon.is_finite()
            && stop.lat.is_finite()
            && (-180.0..=180.0).contains(&stop.lon)
            && (-90.0..=90.0).contains(&stop.lat);
        if !placed || index.contains_key(stop.id.as_str()) {
            continue;
        }
        let (x, y) = to_metres(stop.lon, stop.lat);
        index.insert(stop.id.as_str(), stops.len());
        stops.push(Stop {
            x,
            y,
            lon: stop.lon,
            lat: stop.lat,
            vehicles: 0,
            routes: Vec::new(),
        });
        labels.push(Label {
            name: stop.name.clone().filter(|n| !n.is_empty()),
            code: stop.code.clone().filter(|c| !c.is_empty()),
        });
    }

    let mut serving: Vec<BTreeSet<String>> = vec![BTreeSet::new(); stops.len()];
    let mut seen: HashSet<(usize, usize, &str)> = HashSet::new();
    let mut rides = Vec::new();
    for route in &network.routes {
        if route.name.is_empty() {
            continue;
        }
        let speed = speed_kmh(route.mode.as_deref().unwrap_or("bus")) * 1000.0 / 60.0;
        let at: Vec<Option<usize>> = route
            .stops
            .iter()
            .map(|id| index.get(id.as_str()).copied())
            .collect();
        for stop in at.iter().flatten() {
            serving[*stop].insert(route.name.clone());
        }
        for pair in at.windows(2) {
            let (Some(a), Some(b)) = (pair[0], pair[1]) else {
                continue;
            };
            if a == b || !seen.insert((a, b, route.name.as_str())) {
                continue;
            }
            let metres =
                ((stops[a].x - stops[b].x).powi(2) + (stops[a].y - stops[b].y).powi(2)).sqrt();
            rides.push(Ride {
                from: a,
                to: b,
                route: route.name.clone(),
                minutes: metres * ROUTE_DETOUR / speed,
                runs: 1,
            });
        }
    }
    for (stop, names) in stops.iter_mut().zip(serving) {
        stop.routes = names.into_iter().collect();
    }
    (stops, labels, rides)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::to_lon_lat;

    fn stop(id: &str, east: f64) -> NetStop {
        let (lon, lat) = to_lon_lat(east, 0.0);
        NetStop {
            id: id.into(),
            lon,
            lat,
            name: Some(format!("Stop {id}")),
            code: None,
        }
    }

    fn route(name: &str, mode: &str, stops: &[&str]) -> NetRoute {
        NetRoute {
            name: name.into(),
            mode: Some(mode.into()),
            stops: stops.iter().map(|s| (*s).into()).collect(),
        }
    }

    #[test]
    fn a_line_rides_between_its_consecutive_stops_at_its_modes_speed() {
        let network = Network {
            stops: vec![stop("a", 0.0), stop("b", 2000.0), stop("c", 4000.0)],
            routes: vec![route("M1", "metro", &["a", "b", "c"])],
        };
        let (stops, labels, rides) = build(&network);
        assert_eq!(stops.len(), 3);
        assert_eq!(labels[1].name.as_deref(), Some("Stop b"));
        assert_eq!(rides.len(), 2);
        // 2 km × 1.2 at 40 km/h is 3.6 minutes.
        assert!(
            (rides[0].minutes - 3.6).abs() < 1e-6,
            "{}",
            rides[0].minutes
        );
        assert!(stops.iter().all(|s| s.routes == vec!["M1".to_owned()]));
    }

    #[test]
    fn a_stop_the_register_lacks_breaks_the_line_there_only() {
        let network = Network {
            stops: vec![
                stop("a", 0.0),
                stop("b", 1000.0),
                stop("d", 3000.0),
                stop("e", 4000.0),
            ],
            routes: vec![route("4", "tram", &["a", "b", "x", "d", "e"])],
        };
        let (_, _, rides) = build(&network);
        assert_eq!(
            rides.iter().map(|r| (r.from, r.to)).collect::<Vec<_>>(),
            vec![(0, 1), (2, 3)]
        );
    }

    #[test]
    fn variants_of_one_line_share_their_rides_and_unplaced_or_repeated_stops_are_dropped() {
        let mut nowhere = stop("z", 0.0);
        nowhere.lat = f64::NAN;
        let network = Network {
            stops: vec![stop("a", 0.0), stop("a", 50.0), stop("b", 1000.0), nowhere],
            routes: vec![
                route("550", "bus", &["a", "b"]),
                route("550", "bus", &["a", "b", "z"]),
                route("", "bus", &["a", "b"]),
            ],
        };
        let (stops, _, rides) = build(&network);
        assert_eq!(stops.len(), 2);
        assert_eq!(rides.len(), 1);
    }

    #[test]
    fn nothing_from_nothing() {
        let (stops, labels, rides) = build(&Network::default());
        assert!(stops.is_empty() && labels.is_empty() && rides.is_empty());
    }

    #[test]
    fn every_mode_has_a_speed_and_an_unknown_one_rides_as_a_bus() {
        for mode in ["bus", "tram", "metro", "train", "ferry"] {
            assert!(speed_kmh(mode) > 10.0);
        }
        assert_eq!(speed_kmh("hovercraft"), speed_kmh("bus"));
    }
}
