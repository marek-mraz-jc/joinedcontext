//! Stops and rides from vehicle positions alone (T-3331): the endpoint carries vehicles, not stops
//! or timetables. A stop is a place where readings of at least two vehicles stand still (DBSCAN over
//! the still readings), so a traffic light two buses wait at counts too. A ride is a vehicle's move
//! from one such place to the next on one route, timed by its own readings.

use std::collections::{BTreeMap, BTreeSet};

use crate::dbscan::dbscan;
use crate::geo::to_lon_lat;

/// A reading at or under this speed (m/s) stands still.
pub const STILL: f64 = 0.5;
/// Still readings this close (m) are one place.
pub const EPS: f64 = 35.0;
/// A place needs this many still readings ...
pub const MIN_READINGS: usize = 3;
/// ... from this many vehicles to be a stop.
pub const MIN_VEHICLES: usize = 2;
/// A move between two stops longer than this (ms) is no ride: the vehicle left service or the
/// readings stopped.
pub const MAX_HOP: f64 = 20.0 * 60_000.0;

/// One reading of one vehicle, in metres around Helsinki.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub t: f64,
    pub x: f64,
    pub y: f64,
    pub speed: f64,
    pub route: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stop {
    pub x: f64,
    pub y: f64,
    pub lon: f64,
    pub lat: f64,
    /// The vehicles seen standing here.
    pub vehicles: usize,
    /// The routes of those vehicles, sorted.
    pub routes: Vec<String>,
}

/// One ride between two stops on one route: the median of the times readings show for it.
#[derive(Debug, Clone, PartialEq)]
pub struct Ride {
    pub from: usize,
    pub to: usize,
    pub route: String,
    /// Minutes.
    pub minutes: f64,
    /// How many runs it was seen on.
    pub runs: usize,
}

/// The stops the readings show and the rides between them. `vehicles` holds each vehicle's
/// readings, in any order.
pub fn derive(vehicles: &[Vec<Reading>]) -> (Vec<Stop>, Vec<Ride>) {
    let mut ordered: Vec<Vec<&Reading>> = vehicles
        .iter()
        .map(|readings| {
            let mut sorted: Vec<&Reading> = readings.iter().collect();
            sorted.sort_by(|a, b| a.t.total_cmp(&b.t));
            sorted
        })
        .collect();
    ordered.retain(|readings| !readings.is_empty());

    // Every still reading, with whose it is.
    let mut still: Vec<(usize, usize)> = Vec::new();
    for (v, readings) in ordered.iter().enumerate() {
        for (i, reading) in readings.iter().enumerate() {
            if reading.speed <= STILL {
                still.push((v, i));
            }
        }
    }
    let points: Vec<(f64, f64)> = still
        .iter()
        .map(|&(v, i)| (ordered[v][i].x, ordered[v][i].y))
        .collect();
    let labels = dbscan(&points, EPS, MIN_READINGS);

    // A cluster is a stop when two vehicles stood in it; its place is the mean of its readings.
    let mut members: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (k, label) in labels.iter().enumerate() {
        if let Some(cluster) = label {
            members.entry(*cluster).or_default().push(k);
        }
    }
    let mut stop_of_cluster: BTreeMap<usize, usize> = BTreeMap::new();
    let mut stops: Vec<Stop> = Vec::new();
    for (cluster, ks) in &members {
        let seen: BTreeSet<usize> = ks.iter().map(|&k| still[k].0).collect();
        if seen.len() < MIN_VEHICLES {
            continue;
        }
        let n = ks.len() as f64;
        let x = ks.iter().map(|&k| points[k].0).sum::<f64>() / n;
        let y = ks.iter().map(|&k| points[k].1).sum::<f64>() / n;
        let routes: BTreeSet<String> = ks
            .iter()
            .map(|&k| ordered[still[k].0][still[k].1].route.clone())
            .filter(|route| !route.is_empty())
            .collect();
        let (lon, lat) = to_lon_lat(x, y);
        stop_of_cluster.insert(*cluster, stops.len());
        stops.push(Stop {
            x,
            y,
            lon,
            lat,
            vehicles: seen.len(),
            routes: routes.into_iter().collect(),
        });
    }

    // Each reading's stop, then each vehicle's visits: runs of readings at one stop.
    let mut at: Vec<Vec<Option<usize>>> = ordered.iter().map(|r| vec![None; r.len()]).collect();
    for (k, &(v, i)) in still.iter().enumerate() {
        at[v][i] = labels[k].and_then(|cluster| stop_of_cluster.get(&cluster).copied());
    }
    let mut times: BTreeMap<(usize, usize, String), Vec<f64>> = BTreeMap::new();
    for (v, readings) in ordered.iter().enumerate() {
        // (stop, first reading there, last reading there, route)
        let mut visits: Vec<(usize, f64, f64, &str)> = Vec::new();
        for (i, reading) in readings.iter().enumerate() {
            let Some(stop) = at[v][i] else { continue };
            match visits.last_mut() {
                Some(last) if last.0 == stop => last.2 = reading.t,
                _ => visits.push((stop, reading.t, reading.t, reading.route.as_str())),
            }
        }
        for pair in visits.windows(2) {
            let (a, _, left, route) = pair[0];
            let (b, arrived, _, next_route) = pair[1];
            let hop = arrived - left;
            if a != b && route == next_route && !route.is_empty() && hop > 0.0 && hop <= MAX_HOP {
                times
                    .entry((a, b, route.to_owned()))
                    .or_default()
                    .push(hop / 60_000.0);
            }
        }
    }
    let rides = times
        .into_iter()
        .filter_map(|((from, to, route), minutes)| {
            Some(Ride {
                from,
                to,
                route,
                runs: minutes.len(),
                minutes: crate::median(&minutes)?,
            })
        })
        .collect();
    (stops, rides)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: f64 = 60_000.0;

    fn at(t: f64, x: f64, speed: f64, route: &str) -> Reading {
        Reading {
            t,
            x,
            y: 0.0,
            speed,
            route: route.into(),
        }
    }

    /// A bus on `route` standing at x = 0 for three readings, riding, then standing at x = 1000.
    fn run(start: f64, route: &str, minutes: f64) -> Vec<Reading> {
        vec![
            at(start, 0.0, 0.0, route),
            at(start + 0.25 * MIN, 2.0, 0.0, route),
            at(start + 0.5 * MIN, 1.0, 0.2, route),
            at(start + 1.0 * MIN, 400.0, 9.0, route),
            at(start + 0.5 * MIN + minutes * MIN, 1000.0, 0.0, route),
            at(start + 0.75 * MIN + minutes * MIN, 1001.0, 0.0, route),
            at(start + 1.0 * MIN + minutes * MIN, 999.0, 0.1, route),
        ]
    }

    #[test]
    fn two_vehicles_standing_at_two_places_make_two_stops_and_a_ride() {
        let (stops, rides) = derive(&[run(0.0, "550", 3.0), run(10.0 * MIN, "550", 5.0)]);
        assert_eq!(stops.len(), 2);
        assert!(stops
            .iter()
            .all(|s| s.vehicles == 2 && s.routes == vec!["550".to_owned()]));
        assert_eq!(rides.len(), 1);
        let ride = &rides[0];
        assert_eq!((ride.route.as_str(), ride.runs), ("550", 2));
        assert!(
            (ride.minutes - 4.0).abs() < 1e-9,
            "the median of 3 and 5: {}",
            ride.minutes
        );
        assert!((stops[ride.from].x - 1.0).abs() < 1.0 && (stops[ride.to].x - 1000.0).abs() < 1.0);
    }

    #[test]
    fn one_vehicle_standing_alone_is_no_stop() {
        let (stops, rides) = derive(&[run(0.0, "550", 3.0)]);
        assert!(stops.is_empty() && rides.is_empty());
    }

    #[test]
    fn a_move_too_long_or_onto_another_route_is_no_ride() {
        let mut changed = run(10.0 * MIN, "550", 5.0);
        for reading in changed.iter_mut().skip(4) {
            reading.route = "560".into();
        }
        let (stops, rides) = derive(&[run(0.0, "550", 25.0), changed]);
        assert_eq!(stops.len(), 2);
        assert!(rides.is_empty(), "{rides:?}");
    }

    #[test]
    fn nothing_from_nothing() {
        assert_eq!(derive(&[]), (vec![], vec![]));
        assert_eq!(derive(&[vec![], vec![]]), (vec![], vec![]));
    }

    #[test]
    fn readings_in_any_order_give_the_same_answer() {
        let mut shuffled = run(0.0, "550", 3.0);
        shuffled.reverse();
        assert_eq!(
            derive(&[shuffled, run(10.0 * MIN, "550", 5.0)]),
            derive(&[run(0.0, "550", 3.0), run(10.0 * MIN, "550", 5.0)])
        );
    }
}
