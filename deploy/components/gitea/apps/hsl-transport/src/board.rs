//! The departures and service alerts beside the map (T-3895).
//!
//! Read from the same Endpoint as the fleet, every [`BOARD_SECONDS`], in the simplified
//! representation: the stops that carry departures (T-3891, a curated set of about 230), the
//! headsign of each line and direction from PublicTransportRoute, HSL's service alerts (T-3892) and the
//! stops those alerts name, so the map can place them. A read that fails keeps the last board,
//! as a failed fleet poll keeps the last positions.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::Value;

use crate::PollError;

/// How often the board is read. Departures change every 30 s upstream (T-3891).
pub const BOARD_SECONDS: u64 = 30;
/// A page of the Endpoint; the board pages on until a short one.
const PAGE: usize = 1000;
/// The pages read at most per type: a runaway answer stops here rather than looping.
const PAGES: usize = 5;
/// Ids per lookup of the stops alerts name, so the query string stays short.
const IDS_PER_ASK: usize = 50;
/// `(line, GTFS direction 0 or 1)` to the headsign.
pub type Headsigns = BTreeMap<(String, u8), String>;

/// The alerts this app shows are HSL's; the space also holds Digitraffic's road alerts.
pub const HSL_ALERTS: &str = "https://realtime.hsl.fi/realtime/service-alerts/v2/hsl";

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Board {
    pub departures: Vec<StopBoard>,
    pub alerts: Vec<AlertView>,
}

/// One stop and its next departures, soonest first.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StopBoard {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coordinates: Option<[f64; 2]>,
    pub departures: Vec<Departure>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Departure {
    /// The line as GTFS names it (`2550`, `1009`, `31M1`).
    pub line: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headsign: Option<String>,
    pub expected: String,
    /// scheduled, skipped or canceled.
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delay: Option<i64>,
}

/// One HSL service alert, its text as HSL wrote it: the browser renders it as text, never HTML.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AlertView {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
    #[serde(rename = "validFrom", skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<String>,
    #[serde(rename = "validTo", skip_serializing_if = "Option::is_none")]
    pub valid_to: Option<String>,
    /// The lines it affects, linked or kept as text (`unknownReference`).
    pub lines: Vec<String>,
    /// The stops it affects, by GTFS stop id.
    pub stops: Vec<String>,
    /// Where those stops are, for the ones the space holds.
    pub places: Vec<Place>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Place {
    pub stop: String,
    pub coordinates: [f64; 2],
}

/// Reads the Endpoint once per call. `headsigns` is the cache the caller keeps between reads.
pub async fn read(
    http: &reqwest::Client,
    endpoint: &str,
    headsigns: &mut Headsigns,
) -> Result<Board, PollError> {
    if headsigns.is_empty() {
        *headsigns = headsigns_of(
            &list(
                http,
                endpoint,
                "PublicTransportRoute",
                "route,directionId,headsign",
                &[],
            )
            .await?,
        );
    }
    let stops = list(
        http,
        endpoint,
        "GtfsStop",
        "name,location,departures",
        &[("q", "departures")],
    )
    .await?;
    let alerts = alerts(
        &list(
            http,
            endpoint,
            "Alert",
            "name,description,severity,validFrom,validTo,refRoute,refStop,unknownReference,source",
            &[],
        )
        .await?,
    );
    let named: BTreeSet<String> = alerts.iter().flat_map(stop_urns).collect();
    let mut places = BTreeMap::new();
    let named: Vec<String> = named.into_iter().collect();
    for ids in named.chunks(IDS_PER_ASK) {
        for stop in list(
            http,
            endpoint,
            "GtfsStop",
            "location",
            &[("id", &ids.join(","))],
        )
        .await?
        {
            if let (Some(id), Some(at)) = (stop.get("id").and_then(Value::as_str), point(&stop)) {
                places.insert(id.to_owned(), at);
            }
        }
    }
    Ok(Board {
        departures: boards(&stops, headsigns),
        alerts: alerts
            .into_iter()
            .map(|alert| alert.placed(&places))
            .collect(),
    })
}

/// Every entity of `kind`, simplified, page by page.
async fn list(
    http: &reqwest::Client,
    endpoint: &str,
    kind: &str,
    attrs: &str,
    extra: &[(&str, &str)],
) -> Result<Vec<Value>, PollError> {
    let mut all = Vec::new();
    for page in 0..PAGES {
        let offset = (page * PAGE).to_string();
        let limit = PAGE.to_string();
        let response = http
            .get(format!("{endpoint}ngsi-ld/v1/entities"))
            .query(&[
                ("type", kind),
                ("attrs", attrs),
                ("options", "keyValues"),
                ("limit", &limit),
                ("offset", &offset),
            ])
            .query(extra)
            .header(axum::http::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|_| PollError::Unreachable)?;
        if !response.status().is_success() {
            return Err(PollError::Refused(response.status().as_u16()));
        }
        let body: Value = response.json().await.map_err(|_| PollError::Unreadable)?;
        let entities = body.as_array().ok_or(PollError::Unreadable)?;
        all.extend(entities.iter().cloned());
        if entities.len() < PAGE {
            break;
        }
    }
    Ok(all)
}

/// `(line, GTFS direction 0 or 1)` to the headsign. PublicTransportRoute counts directions from 1, as
/// HSL does; GTFS-RT from 0.
pub fn headsigns_of(routes: &[Value]) -> Headsigns {
    routes
        .iter()
        .filter_map(|route| {
            let line = text(route.get("route")?)?;
            let direction = match route.get("directionId")? {
                Value::Number(n) => n.as_u64()?,
                Value::String(s) => s.parse().ok()?,
                _ => return None,
            };
            let headsign = text(route.get("headsign")?)?;
            (1..=2)
                .contains(&direction)
                .then(|| ((line, (direction - 1) as u8), headsign))
        })
        .collect()
}

/// The stops with at least one departure, by name.
pub fn boards(stops: &[Value], headsigns: &Headsigns) -> Vec<StopBoard> {
    let mut boards: Vec<StopBoard> = stops
        .iter()
        .filter_map(|stop| {
            let departures: Vec<Departure> = stop
                .get("departures")?
                .as_array()?
                .iter()
                .filter_map(|departure| {
                    let line = departure.get("routeId")?.as_str()?.to_owned();
                    let direction = departure.get("directionId").and_then(Value::as_u64);
                    Some(Departure {
                        headsign: direction
                            .and_then(|d| headsigns.get(&(line.clone(), d as u8)).cloned()),
                        expected: departure.get("expected")?.as_str()?.to_owned(),
                        status: departure
                            .get("status")
                            .and_then(Value::as_str)
                            .unwrap_or("scheduled")
                            .to_owned(),
                        delay: departure.get("delay").and_then(Value::as_i64),
                        line,
                    })
                })
                .collect();
            if departures.is_empty() {
                return None;
            }
            let id = stop.get("id")?.as_str()?.to_owned();
            Some(StopBoard {
                name: stop
                    .get("name")
                    .and_then(text)
                    .unwrap_or_else(|| last(&id).to_owned()),
                coordinates: point(stop),
                departures,
                id,
            })
        })
        .collect();
    boards.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    boards
}

/// HSL's alerts, most severe first, then by id; an alert from any other source is left out.
pub fn alerts(entities: &[Value]) -> Vec<AlertView> {
    let rank = |severity: &Option<String>| match severity.as_deref() {
        Some("high") => 0,
        Some("medium") => 1,
        Some("informational") => 2,
        _ => 3,
    };
    let mut alerts: Vec<AlertView> = entities
        .iter()
        .filter(|alert| alert.get("source").and_then(text).as_deref() == Some(HSL_ALERTS))
        .filter_map(|alert| {
            let unknown: Vec<String> = alert
                .get("unknownReference")
                .map(objects)
                .unwrap_or_default();
            let named = |slot: &str, prefix: &str| -> Vec<String> {
                let mut ids: Vec<String> = alert
                    .get(slot)
                    .map(objects)
                    .unwrap_or_default()
                    .iter()
                    .map(|urn| last(urn).to_owned())
                    .collect();
                ids.extend(
                    unknown
                        .iter()
                        .filter_map(|text| text.strip_prefix(prefix))
                        .map(str::to_owned),
                );
                ids.sort();
                ids.dedup();
                ids
            };
            Some(AlertView {
                id: alert.get("id")?.as_str()?.to_owned(),
                title: alert.get("name").and_then(text).unwrap_or_default(),
                body: alert.get("description").and_then(text).unwrap_or_default(),
                severity: alert.get("severity").and_then(text),
                valid_from: alert.get("validFrom").and_then(text),
                valid_to: alert.get("validTo").and_then(text),
                lines: named("refRoute", "route:"),
                stops: named("refStop", "stop:"),
                places: Vec::new(),
            })
        })
        .collect();
    alerts.sort_by(|a, b| {
        rank(&a.severity)
            .cmp(&rank(&b.severity))
            .then_with(|| a.id.cmp(&b.id))
    });
    alerts
}

impl AlertView {
    fn placed(mut self, places: &BTreeMap<String, [f64; 2]>) -> Self {
        self.places = places
            .iter()
            .filter(|(urn, _)| self.stops.iter().any(|stop| stop == last(urn)))
            .map(|(urn, at)| Place {
                stop: last(urn).to_owned(),
                coordinates: *at,
            })
            .collect();
        self
    }
}

/// The GtfsStop URNs of the stops an alert links: same space and domain as the alert itself.
fn stop_urns(alert: &AlertView) -> Vec<String> {
    let Some((prefix, _)) = alert.id.rsplit_once(':') else {
        return Vec::new();
    };
    let space = prefix.trim_start_matches("urn:ngsi-ld:Alert:");
    alert
        .stops
        .iter()
        .map(|stop| format!("urn:ngsi-ld:GtfsStop:{space}:{stop}"))
        .collect()
}

/// A Relationship's object, one or many, in the simplified representation; or a Property's list.
fn objects(value: &Value) -> Vec<String> {
    match value {
        Value::String(one) => vec![one.clone()],
        Value::Array(many) => many
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        Value::Object(fields) => fields
            .get("object")
            .or_else(|| fields.get("value"))
            .map(objects)
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// A text, from a plain value or a language map (English first, then Finnish, then Swedish).
fn text(value: &Value) -> Option<String> {
    let found = match value {
        Value::String(text) => Some(text.as_str()),
        Value::Object(map) => ["en", "fi", "sv"]
            .iter()
            .find_map(|language| map.get(*language).and_then(Value::as_str))
            .or_else(|| map.get("value").and_then(Value::as_str))
            .or_else(|| map.values().find_map(Value::as_str)),
        _ => None,
    }?;
    let found = found.trim();
    (!found.is_empty()).then(|| found.to_owned())
}

fn point(entity: &Value) -> Option<[f64; 2]> {
    let location = entity.get("location")?;
    let location = location.get("value").unwrap_or(location);
    if location.get("type")?.as_str()? != "Point" {
        return None;
    }
    let coordinates = location.get("coordinates")?.as_array()?;
    Some([
        coordinates.first()?.as_f64()?,
        coordinates.get(1)?.as_f64()?,
    ])
}

fn last(urn: &str) -> &str {
    urn.rsplit(':').next().unwrap_or(urn)
}
