// SPDX-License-Identifier: EUPL-1.2
//! The part of `gtfs-realtime.proto` a `VehiclePositions` feed uses, with the reference's field
//! numbers (google/transit at 3c9e7b904b5035349622f03e11851e25c16d1d99, a copy in
//! `tests/gtfsrt/gtfs-realtime.proto` the tests check these against), and the SDM `Vehicle`
//! mapped to it (docs/src/gtfsrt.md section 2). What an Entity does not carry stays absent.

use antares_ops::default_instance as attribute;
use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};

/// `FeedMessage`: the header and one entity per vehicle.
#[derive(Clone, PartialEq, prost::Message)]
pub struct FeedMessage {
    /// Metadata about the feed.
    #[prost(message, required, tag = "1")]
    pub header: FeedHeader,
    /// The vehicles.
    #[prost(message, repeated, tag = "2")]
    pub entity: Vec<FeedEntity>,
}

/// `FeedHeader`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct FeedHeader {
    /// Always `2.0`.
    #[prost(string, required, tag = "1")]
    pub gtfs_realtime_version: String,
    /// Always `FULL_DATASET` (0).
    #[prost(enumeration = "Incrementality", optional, tag = "2")]
    pub incrementality: Option<i32>,
    /// The newest position's time, POSIX seconds.
    #[prost(uint64, optional, tag = "3")]
    pub timestamp: Option<u64>,
}

/// `FeedHeader.Incrementality`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum Incrementality {
    /// Every vehicle, every time.
    FullDataset = 0,
    /// Changes only; never written.
    Differential = 1,
}

/// `FeedEntity` holding a vehicle.
#[derive(Clone, PartialEq, prost::Message)]
pub struct FeedEntity {
    /// The NGSI-LD Entity id.
    #[prost(string, required, tag = "1")]
    pub id: String,
    /// The vehicle's position.
    #[prost(message, optional, tag = "4")]
    pub vehicle: Option<VehiclePosition>,
}

/// `VehiclePosition`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct VehiclePosition {
    /// The trip, when the Entity names its route.
    #[prost(message, optional, tag = "1")]
    pub trip: Option<TripDescriptor>,
    /// Where the vehicle is.
    #[prost(message, optional, tag = "2")]
    pub position: Option<Position>,
    /// When it was there, POSIX seconds.
    #[prost(uint64, optional, tag = "5")]
    pub timestamp: Option<u64>,
    /// Which vehicle.
    #[prost(message, optional, tag = "8")]
    pub vehicle: Option<VehicleDescriptor>,
    /// How full it is, percent.
    #[prost(uint32, optional, tag = "10")]
    pub occupancy_percentage: Option<u32>,
}

/// `Position`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct Position {
    /// WGS 84 degrees.
    #[prost(float, required, tag = "1")]
    pub latitude: f32,
    /// WGS 84 degrees.
    #[prost(float, required, tag = "2")]
    pub longitude: f32,
    /// Degrees clockwise from true north.
    #[prost(float, optional, tag = "3")]
    pub bearing: Option<f32>,
    /// Metres per second.
    #[prost(float, optional, tag = "5")]
    pub speed: Option<f32>,
}

/// `TripDescriptor`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct TripDescriptor {
    /// The GTFS route id.
    #[prost(string, optional, tag = "5")]
    pub route_id: Option<String>,
}

/// `VehicleDescriptor`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct VehicleDescriptor {
    /// The fleet's id of the vehicle.
    #[prost(string, optional, tag = "1")]
    pub id: Option<String>,
    /// The licence plate.
    #[prost(string, optional, tag = "3")]
    pub license_plate: Option<String>,
}

fn text(entity: &Value, name: &str) -> Option<String> {
    let value = attribute(entity, name)?.get("value")?;
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_u64().map(|n| n.to_string()))
}

/// The nearest `f32`: the proto's float fields, from the stored `f64` (no lossy `as`).
fn single(x: f64) -> Option<f32> {
    x.to_string().parse().ok()
}

/// A number Attribute of the Entity and its `unitCode`.
fn number<'a>(entity: &'a Value, name: &str) -> Option<(f64, Option<&'a str>)> {
    let a = attribute(entity, name)?;
    Some((
        a.get("value")?.as_f64()?,
        a.get("unitCode").and_then(Value::as_str),
    ))
}

/// One vehicle as a `FeedEntity` and the time of its position, or `None` without a point and a
/// time (`location` and its `observedAt`).
fn vehicle(entity: &Value) -> Option<(FeedEntity, DateTime<Utc>)> {
    let id = entity.get("id")?.as_str()?;
    let at = attribute(entity, "location")?.get("observedAt")?.as_str()?;
    let at = DateTime::parse_from_rfc3339(at).ok()?.with_timezone(&Utc);
    let geometry = antares_ops::geometry_of(entity, None);
    let point = geometry
        .get("type")
        .filter(|t| *t == "Point")
        .and_then(|_| geometry.get("coordinates")?.as_array());
    let (lon, lat) = match point.map(Vec::as_slice) {
        Some([lon, lat]) => (lon.as_f64()?, lat.as_f64()?),
        _ => return None,
    };
    // SDM speed is km/h unless the Attribute says metres per second (MTS, as HFP writes it)
    let speed = number(entity, "speed")
        .and_then(|(v, unit)| single(if unit == Some("MTS") { v } else { v / 3.6 }));
    let occupancy = number(entity, "occupancy")
        .filter(|(v, unit)| *unit == Some("P1") && (0.0..=100.0).contains(v))
        .and_then(|(v, _)| v.to_string().parse::<u32>().ok());
    let position = VehiclePosition {
        trip: text(entity, "route").map(|r| TripDescriptor { route_id: Some(r) }),
        position: Some(Position {
            latitude: single(lat)?,
            longitude: single(lon)?,
            bearing: number(entity, "heading").and_then(|(v, _)| single(v)),
            speed,
        }),
        timestamp: u64::try_from(at.timestamp()).ok(),
        vehicle: Some(VehicleDescriptor {
            id: Some(text(entity, "fleetVehicleId").unwrap_or_else(|| id.to_owned())),
            license_plate: text(entity, "vehiclePlateIdentifier"),
        }),
        occupancy_percentage: occupancy,
    };
    let feed = FeedEntity {
        id: id.to_owned(),
        vehicle: Some(position),
    };
    Some((feed, at))
}

/// The feed of `vehicles` at `now`: each with a point and a time no older than `window`
/// seconds, the header's time the newest of theirs (else `now`); and how many were left out.
#[must_use]
pub fn message(vehicles: &[Value], now: DateTime<Utc>, window: u32) -> (FeedMessage, usize) {
    let oldest = now
        .checked_sub_signed(Duration::seconds(i64::from(window)))
        .unwrap_or(now);
    let kept: Vec<(FeedEntity, DateTime<Utc>)> = vehicles
        .iter()
        .filter_map(vehicle)
        .filter(|(_, at)| *at >= oldest)
        .collect();
    let newest = kept.iter().map(|(_, at)| *at).max().unwrap_or(now);
    let header = FeedHeader {
        gtfs_realtime_version: "2.0".to_owned(),
        incrementality: Some(Incrementality::FullDataset.into()),
        timestamp: u64::try_from(newest.timestamp()).ok(),
    };
    let skipped = vehicles.len().saturating_sub(kept.len());
    let entity = kept.into_iter().map(|(e, _)| e).collect();
    (FeedMessage { header, entity }, skipped)
}

/// The message in the proto3 JSON form the GTFS Realtime tools read: the proto's field names,
/// a `uint64` as a string, an enum by its name, an absent field absent.
#[must_use]
pub fn json(message: &FeedMessage) -> Value {
    let header = &message.header;
    // an enum value this mirror does not know is written as its number
    let incrementality = header
        .incrementality
        .map(|i| Incrementality::try_from(i).map_or_else(|_| json!(i), |n| json!(n.as_str_name())));
    let head = object([
        (
            "gtfs_realtime_version",
            Some(json!(header.gtfs_realtime_version)),
        ),
        ("incrementality", incrementality),
        ("timestamp", header.timestamp.map(|t| json!(t.to_string()))),
    ]);
    let entities: Vec<Value> = message.entity.iter().map(entity).collect();
    json!({ "header": head, "entity": entities })
}

/// An object of the members that are present.
fn object<const N: usize>(members: [(&str, Option<Value>); N]) -> Value {
    Value::Object(
        members
            .into_iter()
            .filter_map(|(k, v)| Some((k.to_owned(), v?)))
            .collect(),
    )
}

/// A float as the proto's JSON printers write it: its shortest decimal, `60.17` and not the
/// `f64` widening's `60.16999816894531`.
fn float(x: f32) -> Value {
    json!(x.to_string().parse::<f64>().unwrap_or_default())
}

fn entity(e: &FeedEntity) -> Value {
    let vehicle = e.vehicle.as_ref().map(|v| {
        object([
            (
                "trip",
                v.trip
                    .as_ref()
                    .map(|t| object([("route_id", t.route_id.clone().map(Value::from))])),
            ),
            (
                "position",
                v.position.as_ref().map(|p| {
                    object([
                        ("latitude", Some(float(p.latitude))),
                        ("longitude", Some(float(p.longitude))),
                        ("bearing", p.bearing.map(float)),
                        ("speed", p.speed.map(float)),
                    ])
                }),
            ),
            ("timestamp", v.timestamp.map(|t| json!(t.to_string()))),
            (
                "vehicle",
                v.vehicle.as_ref().map(|d| {
                    object([
                        ("id", d.id.clone().map(Value::from)),
                        ("license_plate", d.license_plate.clone().map(Value::from)),
                    ])
                }),
            ),
            (
                "occupancy_percentage",
                v.occupancy_percentage.map(Value::from),
            ),
        ])
    });
    object([("id", Some(json!(e.id))), ("vehicle", vehicle)])
}

impl Incrementality {
    /// The enum value's name in the proto.
    #[must_use]
    pub fn as_str_name(self) -> &'static str {
        match self {
            Self::FullDataset => "FULL_DATASET",
            Self::Differential => "DIFFERENTIAL",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message as _;

    /// The member at JSON pointer `path`, `null` when there is none.
    fn at(v: &Value, path: &str) -> Value {
        v.pointer(path).cloned().unwrap_or(Value::Null)
    }

    /// `base` with each member of `extra` set, or removed when it is `null`.
    fn with(base: Value, extra: &Value) -> Value {
        let mut map: serde_json::Map<String, Value> =
            serde_json::from_value(base).unwrap_or_default();
        for (k, v) in extra.as_object().into_iter().flatten() {
            if v.is_null() {
                map.remove(k);
            } else {
                map.insert(k.clone(), v.clone());
            }
        }
        Value::Object(map)
    }

    fn bus(id: &str, at: &str, extra: &Value) -> Value {
        let base = json!({
            "id": id, "type": "Vehicle",
            "location": {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [24.94, 60.17]}, "observedAt": at},
            "speed": {"type": "Property", "value": 10.5, "unitCode": "MTS", "observedAt": at},
            "heading": {"type": "Property", "value": 90, "unitCode": "DD", "observedAt": at},
            "route": {"type": "Property", "value": "1055", "observedAt": at},
            "occupancy": {"type": "Property", "value": 40, "unitCode": "P1", "observedAt": at},
            "fleetVehicleId": {"type": "Property", "value": "22-1302"},
        });
        with(base, extra)
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-23T10:05:00Z")
            .map(|t| t.with_timezone(&Utc))
            .unwrap_or_default()
    }

    #[test]
    fn a_vehicle_maps_to_its_position_trip_and_descriptor() {
        let (m, skipped) = message(
            &[bus("urn:a", "2026-10-23T10:04:30Z", &json!({}))],
            now(),
            300,
        );
        assert_eq!(skipped, 0);
        assert_eq!(
            json(&m),
            json!({"header": {"gtfs_realtime_version": "2.0", "incrementality": "FULL_DATASET", "timestamp": "1792749870"},
                   "entity": [{"id": "urn:a", "vehicle": {
                       "trip": {"route_id": "1055"},
                       "position": {"latitude": 60.17, "longitude": 24.94, "bearing": 90.0, "speed": 10.5},
                       "timestamp": "1792749870", "vehicle": {"id": "22-1302"}, "occupancy_percentage": 40}}]})
        );
        let decoded = FeedMessage::decode(m.encode_to_vec().as_slice()).ok();
        assert_eq!(decoded, Some(m));
    }

    /// F-ANT5-009: a position whose instances all carry a datasetId is read from the
    /// first one, so the vehicle stays in the feed.
    #[test]
    fn a_named_only_position_still_places_the_vehicle() {
        let when = "2026-10-23T10:04:30Z";
        let named = json!({"location": [{"type": "GeoProperty", "datasetId": "urn:ngsi-ld:Dataset:hsl",
            "value": {"type": "Point", "coordinates": [24.94, 60.17]}, "observedAt": when}]});
        let (m, skipped) = message(&[bus("urn:a", when, &named)], now(), 300);
        assert_eq!(skipped, 0);
        assert_eq!(
            at(&json(&m), "/entity/0/vehicle/position/latitude"),
            json!(60.17)
        );
    }

    #[test]
    fn what_a_vehicle_lacks_stays_absent_and_km_h_become_m_s() {
        let lean = json!({"route": null, "heading": null, "occupancy": {"type": "Property", "value": 140, "unitCode": "P1"},
                          "fleetVehicleId": null, "speed": [{"type": "Property", "value": 36}],
                          "vehiclePlateIdentifier": {"type": "Property", "value": "ABC-123"}});
        let (m, _) = message(&[bus("urn:b", "2026-10-23T10:04:30Z", &lean)], now(), 300);
        let got = json(&m);
        assert_eq!(
            at(&got, "/entity/0/vehicle"),
            json!({"position": {"latitude": 60.17, "longitude": 24.94, "speed": 10.0},
                   "timestamp": "1792749870", "vehicle": {"id": "urn:b", "license_plate": "ABC-123"}})
        );
        let unitless = json!({"occupancy": {"type": "Property", "value": 40}});
        let (m, _) = message(
            &[bus("urn:d", "2026-10-23T10:04:30Z", &unitless)],
            now(),
            300,
        );
        assert_eq!(
            at(&json(&m), "/entity/0/vehicle/occupancy_percentage"),
            Value::Null
        );
        let numbered = json!({"route": {"type": "Property", "value": 55}});
        let (m, _) = message(
            &[bus("urn:c", "2026-10-23T10:04:30Z", &numbered)],
            now(),
            300,
        );
        assert_eq!(
            at(&json(&m), "/entity/0/vehicle/trip"),
            json!({"route_id": "55"})
        );
    }

    #[test]
    fn a_stale_unplaced_or_untimed_vehicle_is_dropped_and_counted() {
        let line = json!({"location": {"type": "GeoProperty", "observedAt": "2026-10-23T10:04:30Z",
            "value": {"type": "LineString", "coordinates": [[24.0, 60.0], [24.1, 60.1]]}}});
        let untimed = json!({"location": {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [24.0, 60.0]}}});
        let badtime = json!({"location": {"type": "GeoProperty", "observedAt": "later",
            "value": {"type": "Point", "coordinates": [24.0, 60.0]}}});
        let vehicles = [
            bus("urn:fresh", "2026-10-23T10:00:00Z", &json!({})),
            bus("urn:stale", "2026-10-23T09:59:59Z", &json!({})),
            bus("urn:line", "2026-10-23T10:00:00Z", &line),
            bus("urn:untimed", "2026-10-23T10:00:00Z", &untimed),
            bus("urn:badtime", "2026-10-23T10:00:00Z", &badtime),
            json!({"type": "Vehicle"}),
        ];
        let (m, skipped) = message(&vehicles, now(), 300);
        assert_eq!(skipped, 5);
        assert_eq!(m.entity.len(), 1);
        assert_eq!(m.header.timestamp, Some(1_792_749_600));
        let (empty, _) = message(&[], now(), 300);
        assert_eq!(empty.header.timestamp, Some(1_792_749_900));
        assert_eq!(at(&json(&empty), "/entity"), json!([]));
    }

    #[test]
    fn an_entity_without_a_position_or_header_without_incrementality_writes_only_what_it_has() {
        let mut m = FeedMessage::default();
        m.header.incrementality = Some(Incrementality::Differential.into());
        m.entity.push(FeedEntity {
            id: "x".to_owned(),
            vehicle: None,
        });
        m.entity.push(FeedEntity {
            id: "y".to_owned(),
            vehicle: Some(VehiclePosition::default()),
        });
        assert_eq!(
            json(&m),
            json!({"header": {"gtfs_realtime_version": "", "incrementality": "DIFFERENTIAL"},
                   "entity": [{"id": "x"}, {"id": "y", "vehicle": {}}]})
        );
        m.header.incrementality = Some(7);
        assert_eq!(at(&json(&m), "/header/incrementality"), 7);
        m.header.incrementality = None;
        assert_eq!(
            at(&json(&m), "/header"),
            json!({"gtfs_realtime_version": ""})
        );
    }
}
