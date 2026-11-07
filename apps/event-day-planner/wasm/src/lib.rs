//! The day planner (T-3329): the events a visitor chose, in the order that lets them attend the
//! most of them with the least walking, each with when to be there, the walk from the last one at
//! 5 km/h, and what does not fit flagged; a suggested day when nothing is chosen; and the plan as
//! an iCalendar file. Built natively for `cargo test` and to WebAssembly for the page, where
//! [`plan`] takes and answers JSON. Times are UTC epoch milliseconds; the page shows them in
//! Helsinki's time zone.

use serde::{Deserialize, Serialize};
#[cfg(feature = "web")]
use wasm_bindgen::prelude::wasm_bindgen;

/// The mean radius of the Earth, in kilometres.
const EARTH_KM: f64 = 6371.0088;
const MINUTE: i64 = 60_000;

/// Up to this many chosen events every order is tried; past it, nearest-in-time then 2-opt.
const EXACT_UP_TO: usize = 8;
/// The most events a plan takes: a day of a person, not a festival's programme.
pub const MAX_CHOSEN: usize = 20;
/// The most events a suggested day holds.
const SUGGESTED: usize = 4;

/// The great-circle distance between two `[lon, lat]` points, in kilometres.
pub fn haversine_km(a: [f64; 2], b: [f64; 2]) -> f64 {
    let (lat1, lat2) = (a[1].to_radians(), b[1].to_radians());
    let dlat = lat2 - lat1;
    let dlon = (b[0] - a[0]).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_KM * h.sqrt().min(1.0).asin()
}

/// One event as the page read it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Event {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub address: String,
    /// When it starts and ends, epoch milliseconds; a missing end makes it a point in time.
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub lon: Option<f64>,
    pub lat: Option<f64>,
}

impl Event {
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

    /// Its time window, the end never before the start.
    fn window(&self) -> Option<(i64, i64)> {
        let start = self.start?;
        Some((start, self.end.unwrap_or(start).max(start)))
    }
}

/// What the visitor chose.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct Settings {
    /// Walking speed, km/h.
    pub walk_kmh: f64,
    /// An event longer than this is a place to drop by (an exhibition, a fair), not one to sit
    /// through: the visitor stays `stay_minutes` at any time it is open.
    pub whole_up_to_minutes: i64,
    pub stay_minutes: i64,
    /// The events chosen, by id, in no particular order; none asks for a suggested day.
    pub chosen: Vec<String>,
    /// When the day starts for the visitor; the first walk is not counted.
    pub day_start: Option<i64>,
    /// The time stamped on the calendar file.
    pub now: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            walk_kmh: 5.0,
            whole_up_to_minutes: 180,
            stay_minutes: 60,
            chosen: Vec::new(),
            day_start: None,
            now: 0,
        }
    }
}

/// How one stop of the day turns out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Fit {
    /// There in time, for as long as planned.
    Ok,
    /// It has started by the time the visitor gets there.
    Late,
    /// It is over, or closes, before the visitor gets there or can stay.
    Missed,
}

/// One stop of the day.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub name: String,
    /// When the visitor is there, and leaves.
    pub begin: i64,
    pub finish: i64,
    /// Walking from the last stop, minutes and kilometres; 0 for the first stop and wherever a
    /// place is missing.
    pub walk_minutes: i64,
    pub walk_km: f64,
    pub fit: Fit,
    /// Minutes late, for a `late` stop.
    pub late_minutes: i64,
    /// Whether it has a place: without one its walk is not counted.
    pub located: bool,
}

/// Two chosen events that take place at the same time; at most one of them can be sat through.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Conflict(pub String, pub String);

/// The day.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Day {
    pub items: Vec<Item>,
    pub conflicts: Vec<Conflict>,
    pub walk_km: f64,
    pub walk_minutes: i64,
    /// Whether the planner chose the events (nothing was chosen).
    pub suggested: bool,
    /// The day as an iCalendar file, the stops that are not missed.
    pub ics: String,
    /// Chosen ids that are no event of the list, or have no start.
    pub unknown: Vec<String>,
}

/// An event ready to plan: its window and how long the visitor stays.
#[derive(Debug, Clone)]
struct Slot<'a> {
    event: &'a Event,
    start: i64,
    end: i64,
    /// Whether the visitor sits through it from its start (else drops by for `stay`).
    whole: bool,
    stay: i64,
}

fn slot<'a>(event: &'a Event, settings: &Settings) -> Option<Slot<'a>> {
    let (start, end) = event.window()?;
    let length = end - start;
    let whole = length <= settings.whole_up_to_minutes.max(0) * MINUTE;
    Some(Slot {
        event,
        start,
        end,
        whole,
        stay: if whole {
            length
        } else {
            settings.stay_minutes.max(1) * MINUTE
        },
    })
}

fn walk(from: Option<[f64; 2]>, to: Option<[f64; 2]>, kmh: f64) -> (f64, i64) {
    match (from, to) {
        (Some(a), Some(b)) if kmh > 0.0 => {
            let km = haversine_km(a, b);
            (km, (km / kmh * 60.0).ceil() as i64)
        }
        _ => (0.0, 0),
    }
}

/// The stops an order yields, and its score: stops in time, then fewer late minutes, then less
/// walking.
fn schedule(
    order: &[usize],
    slots: &[Slot],
    settings: &Settings,
) -> (Vec<Item>, (usize, i64, i64)) {
    let mut items = Vec::with_capacity(order.len());
    let mut clock: Option<i64> = settings.day_start;
    let mut here: Option<[f64; 2]> = None;
    let mut first = true;
    let (mut fitted, mut late_total, mut walked) = (0usize, 0i64, 0i64);
    for &index in order {
        let s = &slots[index];
        let at = s.event.at();
        let (km, minutes) = if first {
            (0.0, 0)
        } else {
            walk(here, at, settings.walk_kmh)
        };
        let arrive = clock.map_or(s.start, |c| c + minutes * MINUTE);
        let begin = arrive.max(s.start);
        let (finish, fit, late) = if s.whole {
            let late = (arrive - s.start).max(0);
            if arrive >= s.end {
                (arrive, Fit::Missed, 0)
            } else if late > 0 {
                (s.end, Fit::Late, late / MINUTE)
            } else {
                (s.end, Fit::Ok, 0)
            }
        } else if begin + s.stay <= s.end {
            (begin + s.stay, Fit::Ok, 0)
        } else {
            (begin, Fit::Missed, 0)
        };
        if fit != Fit::Missed {
            fitted += 1;
            late_total += late;
            walked += minutes;
            clock = Some(finish);
            if at.is_some() {
                here = at;
            }
            first = false;
        }
        items.push(Item {
            id: s.event.id.clone(),
            name: s.event.name.clone(),
            begin,
            finish,
            walk_minutes: minutes,
            walk_km: km,
            fit,
            late_minutes: late,
            located: at.is_some(),
        });
    }
    (items, (fitted, late_total, walked))
}

fn better(a: (usize, i64, i64), b: (usize, i64, i64)) -> bool {
    a.0 > b.0 || (a.0 == b.0 && (a.1, a.2) < (b.1, b.2))
}

/// Every order of `0..n`, by Heap's algorithm, each handed to `visit`.
fn permutations(n: usize, visit: &mut impl FnMut(&[usize])) {
    let mut order: Vec<usize> = (0..n).collect();
    let mut counters = vec![0usize; n];
    visit(&order);
    let mut i = 0;
    while i < n {
        if counters[i] < i {
            if i % 2 == 0 {
                order.swap(0, i);
            } else {
                order.swap(counters[i], i);
            }
            visit(&order);
            counters[i] += 1;
            i = 0;
        } else {
            counters[i] = 0;
            i += 1;
        }
    }
}

/// The best order of `slots`: every one for a few, else by start then 2-opt.
fn best_order(slots: &[Slot], settings: &Settings) -> Vec<usize> {
    let n = slots.len();
    let mut by_start: Vec<usize> = (0..n).collect();
    by_start.sort_by(|&a, &b| {
        (slots[a].start, slots[a].end, &slots[a].event.id).cmp(&(
            slots[b].start,
            slots[b].end,
            &slots[b].event.id,
        ))
    });
    let mut best = by_start.clone();
    let mut best_score = schedule(&best, slots, settings).1;
    if n <= EXACT_UP_TO {
        permutations(n, &mut |order| {
            let score = schedule(order, slots, settings).1;
            if better(score, best_score) {
                best_score = score;
                best = order.to_vec();
            }
        });
        return best;
    }
    let mut improved = true;
    while improved {
        improved = false;
        for i in 0..n {
            for j in i + 1..n {
                best[i..=j].reverse();
                let score = schedule(&best, slots, settings).1;
                if better(score, best_score) {
                    best_score = score;
                    improved = true;
                } else {
                    best[i..=j].reverse();
                }
            }
        }
    }
    best
}

/// Pairs of events sat through from their start whose times overlap.
fn conflicts(slots: &[Slot]) -> Vec<Conflict> {
    let mut found = Vec::new();
    for (i, a) in slots.iter().enumerate() {
        for b in &slots[i + 1..] {
            if a.whole && b.whole && a.start < b.end && b.start < a.end {
                let (x, y) = if a.event.id <= b.event.id {
                    (a, b)
                } else {
                    (b, a)
                };
                found.push(Conflict(x.event.id.clone(), y.event.id.clone()));
            }
        }
    }
    found.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    found
}

/// A day the planner picks from `slots`: the events sat through, by earliest end, each reachable
/// on foot from the last before it starts, at most [`SUGGESTED`].
fn suggest(slots: &[Slot], settings: &Settings) -> Vec<usize> {
    let mut by_end: Vec<usize> = (0..slots.len()).filter(|&i| slots[i].whole).collect();
    by_end.sort_by(|&a, &b| {
        (slots[a].end, slots[a].start, &slots[a].event.id).cmp(&(
            slots[b].end,
            slots[b].start,
            &slots[b].event.id,
        ))
    });
    let mut picked: Vec<usize> = Vec::new();
    let mut clock = settings.day_start;
    let mut here: Option<[f64; 2]> = None;
    for index in by_end {
        if picked.len() == SUGGESTED {
            break;
        }
        let s = &slots[index];
        let minutes = if picked.is_empty() {
            0
        } else {
            walk(here, s.event.at(), settings.walk_kmh).1
        };
        if clock.is_none_or(|c| c + minutes * MINUTE <= s.start) {
            picked.push(index);
            clock = Some(s.end);
            if s.event.at().is_some() {
                here = s.event.at();
            }
        }
    }
    picked
}

/// The day: the chosen events in their best order, or a suggested day when none is chosen.
pub fn run(input: &Input) -> Day {
    let settings = &input.settings;
    let mut unknown = Vec::new();
    let mut chosen: Vec<&str> = Vec::new();
    for id in &settings.chosen {
        if !chosen.contains(&id.as_str()) {
            chosen.push(id);
        }
    }
    let suggested = chosen.is_empty();
    let slots: Vec<Slot> = if suggested {
        input
            .events
            .iter()
            .filter_map(|e| slot(e, settings))
            .collect()
    } else {
        chosen
            .iter()
            .take(MAX_CHOSEN)
            .filter_map(|id| {
                let found = input
                    .events
                    .iter()
                    .find(|e| e.id == *id)
                    .and_then(|e| slot(e, settings));
                if found.is_none() {
                    unknown.push((*id).to_owned());
                }
                found
            })
            .collect()
    };
    let (slots, order) = if suggested {
        let picked = suggest(&slots, settings);
        let slots: Vec<Slot> = picked.iter().map(|&i| slots[i].clone()).collect();
        let order = (0..slots.len()).collect::<Vec<_>>();
        (slots, order)
    } else {
        let order = best_order(&slots, settings);
        (slots, order)
    };
    let (items, _) = schedule(&order, &slots, settings);
    let walk_km = items
        .iter()
        .filter(|i| i.fit != Fit::Missed)
        .map(|i| i.walk_km)
        .sum();
    let walk_minutes = items
        .iter()
        .filter(|i| i.fit != Fit::Missed)
        .map(|i| i.walk_minutes)
        .sum();
    let ics = calendar(&items, &slots, settings.now);
    Day {
        conflicts: conflicts(&slots),
        items,
        walk_km,
        walk_minutes,
        suggested,
        ics,
        unknown,
    }
}

/// `YYYYMMDDTHHMMSSZ` of epoch milliseconds (days from the civil calendar, Hinnant's algorithm).
pub fn ics_time(ms: i64) -> String {
    let seconds = ms.div_euclid(1000);
    let (days, rest) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// A text value of iCalendar: backslash, semicolon, comma and line breaks escaped (RFC 5545 3.3.11).
fn ics_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// A content line folded at 75 octets, never inside a character (RFC 5545 3.1).
fn fold(line: &str) -> String {
    let mut out = String::with_capacity(line.len() + 8);
    let mut used = 0;
    for c in line.chars() {
        if used + c.len_utf8() > 75 {
            out.push_str("\r\n ");
            used = 1;
        }
        out.push(c);
        used += c.len_utf8();
    }
    out.push_str("\r\n");
    out
}

/// The day as an iCalendar file: one VEVENT per stop the visitor makes.
fn calendar(items: &[Item], slots: &[Slot], now: i64) -> String {
    let mut out = String::new();
    for line in [
        "BEGIN:VCALENDAR",
        "VERSION:2.0",
        "PRODID:-//joinedcontext//event-day-planner//EN",
        "CALSCALE:GREGORIAN",
    ] {
        out.push_str(&fold(line));
    }
    for item in items.iter().filter(|i| i.fit != Fit::Missed) {
        let address = slots
            .iter()
            .find(|s| s.event.id == item.id)
            .map(|s| s.event.address.as_str())
            .unwrap_or_default();
        out.push_str(&fold("BEGIN:VEVENT"));
        out.push_str(&fold(&format!(
            "UID:{}@event-day-planner",
            ics_text(&item.id)
        )));
        out.push_str(&fold(&format!("DTSTAMP:{}", ics_time(now))));
        out.push_str(&fold(&format!("DTSTART:{}", ics_time(item.begin))));
        out.push_str(&fold(&format!(
            "DTEND:{}",
            ics_time(item.finish.max(item.begin + MINUTE))
        )));
        out.push_str(&fold(&format!("SUMMARY:{}", ics_text(&item.name))));
        if !address.is_empty() {
            out.push_str(&fold(&format!("LOCATION:{}", ics_text(address))));
        }
        out.push_str(&fold("END:VEVENT"));
    }
    out.push_str(&fold("END:VCALENDAR"));
    out
}

/// What the page sends.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Input {
    pub events: Vec<Event>,
    #[serde(default)]
    pub settings: Settings,
}

/// The page's entry: [`Input`] as JSON in, [`Day`] as JSON out, or `{"error": "…"}` naming what
/// could not be read.
#[cfg_attr(feature = "web", wasm_bindgen)]
pub fn plan(input: &str) -> String {
    match serde_json::from_str::<Input>(input) {
        Ok(input) => serde_json::to_string(&run(&input))
            .unwrap_or_else(|_| r#"{"error":"the day could not be written"}"#.to_owned()),
        Err(err) => serde_json::json!({ "error": format!("the events could not be read: {err}") })
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2030-10-20 00:00 UTC.
    const DAY: i64 = 1_918_684_800_000;
    const H: i64 = 60 * MINUTE;

    fn event(id: &str, start: i64, end: i64, at: Option<[f64; 2]>) -> Event {
        Event {
            id: id.into(),
            name: id.into(),
            address: format!("{id} street 1, Helsinki"),
            start: Some(start),
            end: Some(end),
            lon: at.map(|p| p[0]),
            lat: at.map(|p| p[1]),
        }
    }

    fn chosen(events: Vec<Event>, ids: &[&str]) -> Day {
        run(&Input {
            events,
            settings: Settings {
                chosen: ids.iter().map(|s| (*s).to_owned()).collect(),
                now: DAY,
                ..Settings::default()
            },
        })
    }

    const CATHEDRAL: [f64; 2] = [24.9522, 60.1703];
    const STOA: [f64; 2] = [25.0814, 60.2108];
    const KAMPPI: [f64; 2] = [24.9316, 60.1690];

    #[test]
    fn the_walk_is_the_great_circle_at_five_kmh() {
        let (km, minutes) = walk(Some(CATHEDRAL), Some(KAMPPI), 5.0);
        assert!((km - 1.15).abs() < 0.05, "{km}");
        assert_eq!(minutes, 14);
        assert_eq!(walk(None, Some(KAMPPI), 5.0), (0.0, 0));
        assert_eq!(walk(Some(CATHEDRAL), Some(KAMPPI), 0.0), (0.0, 0));
        assert!(haversine_km([180.0, 0.0], [-180.0, 0.0]) < 1e-6);
    }

    #[test]
    fn the_order_that_fits_most_is_chosen_whatever_order_they_were_picked_in() {
        let events = vec![
            event("evening", DAY + 17 * H, DAY + 18 * H, Some(CATHEDRAL)),
            event("noon", DAY + 12 * H, DAY + 13 * H, Some(KAMPPI)),
            event("afternoon", DAY + 14 * H, DAY + 15 * H, Some(CATHEDRAL)),
        ];
        let day = chosen(events, &["evening", "afternoon", "noon"]);
        let order: Vec<&str> = day.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(order, ["noon", "afternoon", "evening"]);
        assert!(day.items.iter().all(|i| i.fit == Fit::Ok));
        assert_eq!(day.items[1].walk_minutes, 14);
        assert_eq!(day.items[2].walk_minutes, 0);
        assert!(day.conflicts.is_empty());
        assert!(!day.suggested);
    }

    #[test]
    fn overlapping_events_are_flagged_and_the_later_one_is_late() {
        let events = vec![
            event("a", DAY + 12 * H, DAY + 14 * H, Some(CATHEDRAL)),
            event("b", DAY + 13 * H, DAY + 15 * H, Some(CATHEDRAL)),
        ];
        let day = chosen(events, &["a", "b"]);
        assert_eq!(day.conflicts, [Conflict("a".into(), "b".into())]);
        let b = day.items.iter().find(|i| i.id == "b").expect("b");
        assert_eq!((b.fit, b.late_minutes), (Fit::Late, 60));
    }

    #[test]
    fn a_long_walk_makes_the_next_event_late_and_a_closed_one_missed() {
        let events = vec![
            event("cathedral", DAY + 12 * H, DAY + 13 * H, Some(CATHEDRAL)),
            // Stoa is about 9 km away: almost two hours on foot.
            event("stoa", DAY + 13 * H, DAY + 14 * H, Some(STOA)),
        ];
        let day = chosen(events.clone(), &["cathedral", "stoa"]);
        // Either order loses one of them; the planner keeps one in time.
        assert_eq!(day.items.iter().filter(|i| i.fit == Fit::Ok).count(), 1);
        let missed = vec![
            event("first", DAY + 12 * H, DAY + 13 * H, Some(CATHEDRAL)),
            event(
                "short",
                DAY + 13 * H + 5 * MINUTE,
                DAY + 13 * H + 30 * MINUTE,
                Some(STOA),
            ),
        ];
        let day = chosen(missed, &["first", "short"]);
        assert!(day.items.iter().any(|i| i.fit == Fit::Missed));
        assert!(
            day.ics.matches("BEGIN:VEVENT").count() == 1,
            "a missed stop is not in the calendar"
        );
    }

    #[test]
    fn a_long_event_is_dropped_by_for_an_hour_when_it_suits() {
        let events = vec![
            event("exhibition", DAY + 10 * H, DAY + 18 * H, Some(CATHEDRAL)),
            event("concert", DAY + 12 * H, DAY + 13 * H, Some(CATHEDRAL)),
        ];
        let day = chosen(events, &["exhibition", "concert"]);
        assert!(
            day.items.iter().all(|i| i.fit == Fit::Ok),
            "{:?}",
            day.items
        );
        let exhibition = day
            .items
            .iter()
            .find(|i| i.id == "exhibition")
            .expect("in the day");
        assert_eq!(exhibition.finish - exhibition.begin, H);
        assert!(
            day.conflicts.is_empty(),
            "a place to drop by conflicts with nothing"
        );
    }

    #[test]
    fn an_event_with_no_place_counts_no_walk_and_says_so() {
        let events = vec![
            event("here", DAY + 12 * H, DAY + 13 * H, Some(CATHEDRAL)),
            event("nowhere", DAY + 13 * H, DAY + 14 * H, None),
            event("there", DAY + 15 * H, DAY + 16 * H, Some(KAMPPI)),
        ];
        let day = chosen(events, &["here", "nowhere", "there"]);
        let nowhere = day
            .items
            .iter()
            .find(|i| i.id == "nowhere")
            .expect("in the day");
        assert!(!nowhere.located);
        assert_eq!(nowhere.walk_minutes, 0);
        // The walk to Kamppi is counted from the cathedral, the last place known.
        assert_eq!(
            day.items
                .iter()
                .find(|i| i.id == "there")
                .map(|i| i.walk_minutes),
            Some(14)
        );
    }

    #[test]
    fn nothing_chosen_suggests_a_day_that_fits() {
        let events = vec![
            event("a", DAY + 10 * H, DAY + 11 * H, Some(CATHEDRAL)),
            event(
                "b",
                DAY + 10 * H + 30 * MINUTE,
                DAY + 11 * H + 30 * MINUTE,
                Some(CATHEDRAL),
            ),
            event("c", DAY + 11 * H + 5 * MINUTE, DAY + 12 * H, Some(KAMPPI)),
            event("d", DAY + 11 * H + 20 * MINUTE, DAY + 12 * H, Some(KAMPPI)),
            event("fair", DAY + 9 * H, DAY + 20 * H, Some(KAMPPI)),
        ];
        let day = run(&Input {
            events,
            settings: Settings::default(),
        });
        assert!(day.suggested);
        let ids: Vec<&str> = day.items.iter().map(|i| i.id.as_str()).collect();
        // a ends 11:00, 14 minutes to Kamppi: c at 11:05 is out of reach, d at 11:20 is not.
        assert_eq!(ids, ["a", "d"]);
        assert!(day.items.iter().all(|i| i.fit == Fit::Ok));
    }

    #[test]
    fn no_events_and_unknown_choices_plan_an_empty_day() {
        let day = chosen(Vec::new(), &[]);
        assert!(day.items.is_empty() && day.suggested);
        assert!(
            day.ics.starts_with("BEGIN:VCALENDAR\r\n") && day.ics.ends_with("END:VCALENDAR\r\n")
        );
        let mut undated = event("undated", 0, 0, None);
        undated.start = None;
        let day = chosen(
            vec![undated, event("one", DAY, DAY + H, None)],
            &["gone", "undated", "one", "one"],
        );
        assert_eq!(day.unknown, ["gone", "undated"]);
        assert_eq!(day.items.len(), 1, "a repeated choice is one stop");
    }

    #[test]
    fn many_choices_are_planned_by_2_opt_and_capped() {
        let events: Vec<Event> = (0..30)
            .map(|i| {
                event(
                    &format!("e{i:02}"),
                    DAY + i64::from(i) * H,
                    DAY + i64::from(i) * H + 30 * MINUTE,
                    Some(CATHEDRAL),
                )
            })
            .collect();
        let ids: Vec<String> = (0..30).rev().map(|i| format!("e{i:02}")).collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let day = chosen(events, &refs);
        assert_eq!(day.items.len(), MAX_CHOSEN);
        assert!(day.items.iter().all(|i| i.fit == Fit::Ok));
        assert!(day.items.windows(2).all(|w| w[0].begin < w[1].begin));
    }

    #[test]
    fn the_calendar_is_rfc_5545() {
        assert_eq!(
            ics_time(DAY + 17 * H + 30 * MINUTE + 5_000),
            "20301020T173005Z"
        );
        assert_eq!(ics_time(0), "19700101T000000Z");
        assert_eq!(ics_time(951_782_400_000), "20000229T000000Z");
        assert_eq!(
            ics_text("Jazz, blues; and\\more\nnext"),
            "Jazz\\, blues\\; and\\\\more\\nnext"
        );
        let long = format!("SUMMARY:{}", "ä".repeat(60));
        let folded = fold(&long);
        assert!(
            folded.split("\r\n").all(|line| line.len() <= 75),
            "{folded:?}"
        );
        assert_eq!(folded.replace("\r\n ", "").trim_end(), long);
        let day = chosen(
            vec![event(
                "Jazz, at Stoa",
                DAY + 17 * H,
                DAY + 18 * H,
                Some(STOA),
            )],
            &["Jazz, at Stoa"],
        );
        assert!(day.ics.contains("DTSTART:20301020T170000Z\r\n"));
        assert!(day.ics.contains("SUMMARY:Jazz\\, at Stoa\r\n"));
        assert!(day
            .ics
            .contains("LOCATION:Jazz\\, at Stoa street 1\\, Helsinki\r\n"));
        assert!(day.ics.contains("DTSTAMP:20301020T000000Z\r\n"));
    }

    #[test]
    fn the_entry_answers_json_and_names_what_it_could_not_read() {
        let answer: serde_json::Value =
            serde_json::from_str(&plan(r#"{"events":[]}"#)).expect("json");
        assert_eq!(answer["items"], serde_json::json!([]));
        let refused: serde_json::Value = serde_json::from_str(&plan("[]")).expect("json");
        assert!(refused["error"]
            .as_str()
            .unwrap_or_default()
            .starts_with("the events could not be read"));
    }
}
