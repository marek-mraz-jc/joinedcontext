//! How far one gets from a point in a number of minutes, walking to a stop, waiting, riding,
//! changing on foot and walking on (T-3331). Dijkstra over two states per stop, on foot there and
//! aboard a route there, so staying aboard costs no second wait. Every hexagon a walk from the
//! point or from a reached stop covers in time gets the earliest minute it is reached at.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use crate::geo::{hex_centre, hex_of};
use crate::stops::{Ride, Stop};

/// Walking speed, metres a minute (4.5 km/h).
pub const WALK_PER_MINUTE: f64 = 75.0;
/// Streets are not straight: a walk is this much longer than the straight line.
pub const DETOUR: f64 = 1.3;
/// Changing between two stops on foot only this close (m).
pub const TRANSFER: f64 = 300.0;

/// Minutes on foot over a straight-line distance in metres.
pub fn walk_minutes(metres: f64) -> f64 {
    metres * DETOUR / WALK_PER_MINUTE
}

/// What reaching costs: the wait at each boarding, and how far to look.
#[derive(Debug, Clone, Copy)]
pub struct Rules {
    pub wait: f64,
    pub limit: f64,
}

/// The earliest minute each stop is reached on foot from the point; `None` beyond the limit.
pub fn stops_reached(
    origin: (f64, f64),
    stops: &[Stop],
    rides: &[Ride],
    rules: Rules,
) -> Vec<Option<f64>> {
    let n = stops.len();
    // States: 0..n on foot at a stop; then one per (stop, route) a ride leaves or reaches.
    let mut aboard: HashMap<(usize, &str), usize> = HashMap::new();
    for ride in rides {
        for stop in [ride.from, ride.to] {
            let next = n + aboard.len();
            aboard.entry((stop, ride.route.as_str())).or_insert(next);
        }
    }
    let mut edges: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n + aboard.len()];
    for (&(stop, _), &state) in &aboard {
        edges[stop].push((state, rules.wait));
        edges[state].push((stop, 0.0));
    }
    for ride in rides {
        let (a, b) = (
            aboard[&(ride.from, ride.route.as_str())],
            aboard[&(ride.to, ride.route.as_str())],
        );
        edges[a].push((b, ride.minutes));
    }
    // The changes on foot: each stop against the stops of its own and the neighbouring cells of a
    // TRANSFER-sized grid, so 8000 stops cost a few neighbours each, not every pair.
    let cell = |stop: &Stop| {
        (
            (stop.x / TRANSFER).floor() as i64,
            (stop.y / TRANSFER).floor() as i64,
        )
    };
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, stop) in stops.iter().enumerate() {
        grid.entry(cell(stop)).or_default().push(i);
    }
    for (i, stop) in stops.iter().enumerate() {
        let (cx, cy) = cell(stop);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for &j in grid
                    .get(&(cx + dx, cy + dy))
                    .map(Vec::as_slice)
                    .unwrap_or(&[])
                {
                    let d = ((stop.x - stops[j].x).powi(2) + (stop.y - stops[j].y).powi(2)).sqrt();
                    if i != j && d <= TRANSFER {
                        edges[i].push((j, walk_minutes(d)));
                    }
                }
            }
        }
    }

    // Costs in whole milliseconds of a minute's thousandth, so the heap orders integers.
    let scale = |minutes: f64| (minutes * 1000.0).round() as u64;
    let mut best: Vec<Option<u64>> = vec![None; edges.len()];
    let mut heap = BinaryHeap::new();
    for (i, stop) in stops.iter().enumerate() {
        let minutes =
            walk_minutes(((stop.x - origin.0).powi(2) + (stop.y - origin.1).powi(2)).sqrt());
        if minutes <= rules.limit {
            let cost = scale(minutes);
            if best[i].is_none_or(|b| cost < b) {
                best[i] = Some(cost);
                heap.push(Reverse((cost, i)));
            }
        }
    }
    let limit = scale(rules.limit);
    while let Some(Reverse((cost, state))) = heap.pop() {
        if best[state].is_some_and(|b| cost > b) {
            continue;
        }
        for &(next, minutes) in &edges[state] {
            let reached = cost + scale(minutes);
            if reached <= limit && best[next].is_none_or(|b| reached < b) {
                best[next] = Some(reached);
                heap.push(Reverse((reached, next)));
            }
        }
    }
    best[..n]
        .iter()
        .map(|c| c.map(|c| c as f64 / 1000.0))
        .collect()
}

/// The hexagons (axial q, r) reached within the limit, each with its earliest minute: on foot from
/// the point, or on foot from a stop reached earlier.
pub fn hexes_reached(
    origin: (f64, f64),
    stops: &[Stop],
    at: &[Option<f64>],
    size: f64,
    limit: f64,
) -> HashMap<(i64, i64), f64> {
    let mut reached: HashMap<(i64, i64), f64> = HashMap::new();
    let mut spread = |(x, y): (f64, f64), start: f64| {
        let radius = (limit - start) * WALK_PER_MINUTE / DETOUR;
        if radius < 0.0 {
            return;
        }
        // Every hexagon whose centre is inside the walk's circle; the rings around the centre's
        // hexagon out to the radius hold them all.
        let (cq, cr) = hex_of(x, y, size);
        let rings = (radius / (size * 3f64.sqrt())).ceil() as i64 + 1;
        for dq in -rings..=rings {
            for dr in (-rings).max(-dq - rings)..=rings.min(-dq + rings) {
                let (q, r) = (cq + dq, cr + dr);
                let (hx, hy) = hex_centre(q, r, size);
                let minutes = start + walk_minutes(((hx - x).powi(2) + (hy - y).powi(2)).sqrt());
                if minutes <= limit {
                    let slot = reached.entry((q, r)).or_insert(minutes);
                    if minutes < *slot {
                        *slot = minutes;
                    }
                }
            }
        }
    };
    spread(origin, 0.0);
    for (stop, minutes) in stops.iter().zip(at) {
        if let Some(minutes) = minutes {
            spread((stop.x, stop.y), *minutes);
        }
    }
    reached
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stop(x: f64, y: f64) -> Stop {
        Stop {
            x,
            y,
            lon: 0.0,
            lat: 0.0,
            vehicles: 2,
            routes: vec!["1".into()],
        }
    }

    fn ride(from: usize, to: usize, route: &str, minutes: f64) -> Ride {
        Ride {
            from,
            to,
            route: route.into(),
            minutes,
            runs: 1,
        }
    }

    const RULES: Rules = Rules {
        wait: 5.0,
        limit: 30.0,
    };

    #[test]
    fn walking_takes_the_detour_at_four_and_a_half_kilometres_an_hour() {
        assert!((walk_minutes(750.0) - 13.0).abs() < 1e-9);
        assert_eq!(walk_minutes(0.0), 0.0);
    }

    #[test]
    fn a_ride_takes_the_walk_the_wait_and_the_ride() {
        // 150 m to the first stop (2.6 min), 5 min wait, 6 min ride to a stop 6 km away.
        let stops = [stop(150.0, 0.0), stop(6150.0, 0.0)];
        let at = stops_reached((0.0, 0.0), &stops, &[ride(0, 1, "1", 6.0)], RULES);
        assert!((at[0].expect("walked") - 2.6).abs() < 1e-9);
        assert!((at[1].expect("ridden") - 13.6).abs() < 1e-9);
    }

    #[test]
    fn staying_aboard_costs_no_second_wait_and_a_change_costs_one() {
        let stops = [
            stop(0.0, 0.0),
            stop(3000.0, 0.0),
            stop(6000.0, 0.0),
            stop(6000.0, 3000.0),
        ];
        let rides = [
            ride(0, 1, "1", 4.0),
            ride(1, 2, "1", 4.0),
            ride(2, 3, "2", 4.0),
        ];
        let at = stops_reached((0.0, 0.0), &stops, &rides, RULES);
        assert_eq!(at[2], Some(13.0), "one wait, two rides");
        assert_eq!(at[3], Some(22.0), "a second wait for the change");
    }

    #[test]
    fn a_change_on_foot_is_walked_and_nothing_beyond_the_limit_is_reached() {
        let stops = [
            stop(0.0, 0.0),
            stop(5000.0, 0.0),
            stop(5200.0, 0.0),
            stop(9000.0, 0.0),
        ];
        let rides = [ride(0, 1, "1", 3.0), ride(2, 3, "2", 30.0)];
        let at = stops_reached((0.0, 0.0), &stops, &rides, RULES);
        let walked = 8.0 + walk_minutes(200.0);
        assert!(
            (at[2].expect("changed on foot") - walked).abs() < 1e-3,
            "costs are kept in thousandths of a minute"
        );
        assert_eq!(at[3], None, "past 30 minutes");
    }

    #[test]
    fn a_ride_only_goes_its_way() {
        let stops = [stop(0.0, 0.0), stop(6000.0, 0.0)];
        let at = stops_reached((6000.0, 0.0), &stops, &[ride(0, 1, "1", 1.0)], RULES);
        assert_eq!(at[1], Some(0.0));
        assert_eq!(
            at[0], None,
            "the ride runs from 0 to 1 only, and 6 km is past a 30-minute walk"
        );
    }

    #[test]
    fn no_stops_no_rides_and_an_empty_answer() {
        assert!(stops_reached((0.0, 0.0), &[], &[], RULES).is_empty());
    }

    #[test]
    fn the_hexagons_fill_a_walking_circle_and_stops_add_their_own() {
        let size = 100.0;
        let alone = hexes_reached((0.0, 0.0), &[], &[], size, 10.0);
        // A 10-minute walk covers 750 / 1.3 ≈ 577 m around the point.
        let radius = 10.0 * WALK_PER_MINUTE / DETOUR;
        for (&(q, r), &minutes) in &alone {
            let (x, y) = hex_centre(q, r, size);
            assert!((x * x + y * y).sqrt() <= radius + 1e-6 && minutes <= 10.0);
        }
        let disc = std::f64::consts::PI * radius * radius;
        let hexagon = 1.5 * 3f64.sqrt() * size * size;
        let expected = disc / hexagon;
        assert!(
            ((alone.len() as f64) - expected).abs() < 0.15 * expected,
            "{} against {expected}",
            alone.len()
        );
        assert_eq!(alone.get(&(0, 0)), Some(&0.0));

        let far = [stop(5000.0, 0.0)];
        let with_stop = hexes_reached((0.0, 0.0), &far, &[Some(4.0)], size, 10.0);
        let (q, r) = hex_of(5000.0, 0.0, size);
        assert!((with_stop[&(q, r)] - 4.0).abs() < 1.0);
        assert!(with_stop.len() > alone.len());
    }
}
