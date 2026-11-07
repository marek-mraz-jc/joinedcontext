// SPDX-License-Identifier: EUPL-1.2
//! The view's `gbfs` outputs read, and the SDM `BikeHireDockingStation` mapped to the GBFS 3.0
//! files (docs/src/gbfs.md section 2). What an Entity does not carry stays out of the file; a
//! station missing a field GBFS requires is left out of that file and counted.

use antares_ops::default_instance as attribute;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The most named outputs one view lists (`views::MAX_OUTPUTS`).
const MAX_OUTPUTS: usize = 20;

/// The longest `ttl` an output may set, in seconds: a day.
const MAX_TTL: u32 = 86_400;

/// One `representations.gbfs` output as written (docs/src/gbfs.md section 1).
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Written {
    name: String,
    system_id: Option<String>,
    language: String,
    title: BTreeMap<String, String>,
    timezone: String,
    feed_contact_email: String,
    opening_hours: String,
    #[serde(default = "default_ttl")]
    ttl: u32,
}

fn default_ttl() -> u32 {
    60
}

/// What `system_information.json` says of the system, and the files' `ttl`.
pub(crate) struct System {
    name: String,
    id: Option<String>,
    language: String,
    title: BTreeMap<String, String>,
    timezone: String,
    email: String,
    opening_hours: String,
    pub(crate) ttl: u32,
}

impl System {
    /// Output `name` of the view's `gbfs` member: `None` when the view lists no such output,
    /// `Err` when the member does not hold (more than [`MAX_OUTPUTS`], a repeated name, a
    /// field missing, unknown or out of bounds).
    pub(crate) fn of(written: Option<&Value>, name: &str) -> Result<Option<Self>, &'static str> {
        let Some(written) = written else {
            return Ok(None);
        };
        let outputs = Vec::<Written>::deserialize(written).map_err(|_| "not a list of outputs")?;
        let names: BTreeSet<&str> = outputs.iter().map(|o| o.name.as_str()).collect();
        if outputs.len() > MAX_OUTPUTS || names.len() != outputs.len() {
            return Err("more than 20 outputs or a name twice");
        }
        let mut found = None;
        for o in outputs {
            let system = Self::checked(o)?;
            if system.name == name {
                found = Some(system);
            }
        }
        Ok(found)
    }

    fn checked(o: Written) -> Result<Self, &'static str> {
        let word = |s: &str, extra: &str| {
            (1..=64).contains(&s.len())
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
        };
        let holds = word(&o.name, "-")
            && o.name.bytes().all(|b| !b.is_ascii_uppercase())
            && o.system_id.as_deref().is_none_or(|s| word(s, "-_.:"))
            && language(&o.language)
            && (1..=10).contains(&o.title.len())
            && o.title.iter().all(|(l, t)| language(l) && text(t))
            && o.timezone.split('/').all(|s| word(s, "_+-"))
            && email(&o.feed_contact_email)
            && text(&o.opening_hours)
            && o.ttl <= MAX_TTL;
        if !holds {
            return Err("a field out of bounds");
        }
        Ok(Self {
            name: o.name,
            id: o.system_id,
            language: o.language,
            title: o.title,
            timezone: o.timezone,
            email: o.feed_contact_email,
            opening_hours: o.opening_hours,
            ttl: o.ttl,
        })
    }
}

/// An IETF BCP 47 tag as GBFS takes it: `fi`, `en-GB`.
fn language(tag: &str) -> bool {
    let (base, region) = tag.split_once('-').unwrap_or((tag, "AA"));
    (2..=3).contains(&base.len())
        && base.bytes().all(|b| b.is_ascii_lowercase())
        && region.len() == 2
        && region.bytes().all(|b| b.is_ascii_uppercase())
}

/// A text shown to riders: 1 to 200 characters, no control character.
fn text(t: &str) -> bool {
    (1..=200).contains(&t.chars().count()) && !t.chars().any(char::is_control)
}

/// An address a reader can write to: one `@`, a dotted domain, no space, at most 254 bytes.
fn email(address: &str) -> bool {
    let Some((local, domain)) = address.split_once('@') else {
        return false;
    };
    address.len() <= 254
        && !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !address.chars().any(|c| c.is_whitespace() || c.is_control())
        && !domain.contains('@')
}

fn value<'a>(entity: &'a Value, name: &str) -> Option<&'a Value> {
    attribute(entity, name)?.get("value")
}

/// When the station last reported its bikes, in UTC.
fn reported(entity: &Value) -> Option<DateTime<Utc>> {
    let at = attribute(entity, "availableBikeNumber")?
        .get("observedAt")?
        .as_str()?;
    DateTime::parse_from_rfc3339(at)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// A station's name per language: a `LanguageProperty`'s map, or a `Property`'s text in the
/// output's `language`. Translations under a tag GBFS refuses are left out.
fn names(entity: &Value, fallback: &str) -> Vec<(String, String)> {
    let Some(name) = attribute(entity, "name") else {
        return Vec::new();
    };
    let texts: Vec<(String, String)> = match (name.get("languageMap"), name.get("value")) {
        (Some(Value::Object(map)), _) => map
            .iter()
            .filter_map(|(l, t)| Some((l.clone(), t.as_str()?.to_owned())))
            .collect(),
        (_, Some(Value::String(t))) => vec![(fallback.to_owned(), t.clone())],
        _ => Vec::new(),
    };
    texts
        .into_iter()
        .filter(|(l, t)| language(l) && text(t))
        .collect()
}

fn localised(texts: impl IntoIterator<Item = (String, String)>) -> Value {
    texts
        .into_iter()
        .map(|(language, text)| json!({ "text": text, "language": language }))
        .collect()
}

/// `station_information`'s record of a station, or `None` without a name or a point.
fn information(entity: &Value, system: &System) -> Option<Value> {
    let names = names(entity, &system.language);
    let geometry = antares_ops::geometry_of(entity, None);
    let point = geometry
        .get("type")
        .filter(|t| *t == "Point")
        .and_then(|_| geometry.get("coordinates")?.as_array());
    let (lon, lat) = match point.map(Vec::as_slice) {
        Some([lon, lat]) => (lon.as_f64()?, lat.as_f64()?),
        _ => return None,
    };
    if names.is_empty() {
        return None;
    }
    let mut station = Map::new();
    station.insert("station_id".into(), entity.get("id")?.clone());
    station.insert("name".into(), localised(names));
    station.insert("lat".into(), json!(lat));
    station.insert("lon".into(), json!(lon));
    if let Some(capacity) = value(entity, "totalSlotNumber").and_then(Value::as_u64) {
        station.insert("capacity".into(), json!(capacity));
    }
    Some(Value::Object(station))
}

/// `station_status`'s record of a station, or `None` without its bikes, status or report time.
fn status(entity: &Value) -> Option<Value> {
    let bikes = value(entity, "availableBikeNumber")?.as_u64()?;
    // SDM status: every state but outOfService rents and takes bikes back
    let open = value(entity, "status")?.as_str()? != "outOfService";
    let mut station = Map::new();
    station.insert("station_id".into(), entity.get("id")?.clone());
    station.insert("num_vehicles_available".into(), json!(bikes));
    station.insert("is_installed".into(), json!(true));
    station.insert("is_renting".into(), json!(open));
    station.insert("is_returning".into(), json!(open));
    station.insert("last_reported".into(), json!(stamp(reported(entity)?)));
    if let Some(docks) = value(entity, "freeSlotNumber").and_then(Value::as_u64) {
        station.insert("num_docks_available".into(), json!(docks));
    }
    Some(Value::Object(station))
}

fn stamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// File `file` (one of [`crate::FILES`]) of `system` over `stations` read through view `view`,
/// its sibling files under `url`, and how many stations it left out. Every file of one read
/// carries the same `last_updated`: the newest station report, else now.
pub(crate) fn file(
    file: &str,
    system: &System,
    view: &str,
    stations: &[Value],
    url: &str,
) -> (Value, usize) {
    let last = stations
        .iter()
        .filter_map(reported)
        .max()
        .unwrap_or_else(Utc::now);
    let records = |f: fn(&Value, &System) -> Option<Value>| {
        let kept: Vec<Value> = stations.iter().filter_map(|s| f(s, system)).collect();
        let skipped = stations.len().saturating_sub(kept.len());
        (json!({ "stations": kept }), skipped)
    };
    let (data, skipped) = match file {
        "gbfs" => {
            let feeds: Vec<Value> = crate::FILES
                .iter()
                .skip(1)
                .map(|f| json!({ "name": f, "url": format!("{url}/{f}.json") }))
                .collect();
            (json!({ "feeds": feeds }), 0)
        }
        "system_information" => {
            let mut languages: BTreeSet<String> = system.title.keys().cloned().collect();
            languages.insert(system.language.clone());
            for s in stations {
                languages.extend(names(s, &system.language).into_iter().map(|(l, _)| l));
            }
            let id = system
                .id
                .clone()
                .unwrap_or_else(|| format!("{view}-{}", system.name));
            let data = json!({
                "system_id": id,
                "languages": languages,
                "name": localised(system.title.clone()),
                "opening_hours": system.opening_hours,
                "feed_contact_email": system.email,
                "timezone": system.timezone,
            });
            (data, 0)
        }
        "station_information" => records(information),
        _ => records(|s, _| status(s)),
    };
    let body = json!({
        "last_updated": stamp(last),
        "ttl": system.ttl,
        "version": "3.0",
        "data": data,
    });
    (body, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The member at JSON pointer `path`, `null` when there is none.
    fn at(v: &Value, path: &str) -> Value {
        v.pointer(path).cloned().unwrap_or(Value::Null)
    }

    /// `base` with each member of `extra` set, or removed when it is `null`.
    fn with(base: Value, extra: &Value) -> Value {
        let mut map: Map<String, Value> = serde_json::from_value(base).unwrap_or_default();
        for (k, v) in extra.as_object().into_iter().flatten() {
            if v.is_null() {
                map.remove(k);
            } else {
                map.insert(k.clone(), v.clone());
            }
        }
        Value::Object(map)
    }

    fn output(extra: &Value) -> Value {
        let base = json!({
            "name": "bikes", "language": "fi", "title": {"fi": "Kaupunkipyörät", "en": "City bikes"},
            "timezone": "Europe/Helsinki", "feedContactEmail": "open@hel.example", "openingHours": "Apr 1-Oct 31"
        });
        with(base, extra)
    }

    #[test]
    fn an_output_holds_or_the_member_is_refused_whole() -> Result<(), String> {
        let ok = System::of(Some(&json!([output(&json!({}))])), "bikes")?.ok_or("absent")?;
        assert_eq!(ok.ttl, 60);
        let other = System::of(Some(&json!([output(&json!({}))])), "cars")?;
        assert!(other.is_none());
        assert!(System::of(None, "bikes")?.is_none());
        let bad = [
            json!({"name": "Bikes"}),
            json!({"name": ""}),
            json!({"systemId": "a b"}),
            json!({"language": "FI"}),
            json!({"language": "fi-fi"}),
            json!({"language": "f"}),
            json!({"title": {}}),
            json!({"title": {"fi": ""}}),
            json!({"title": {"FI": "Pyörät"}}),
            json!({"language": "fi-FIN"}),
            json!({"feedContactEmail": format!("{}@hel.example", "a".repeat(250))}),
            json!({"title": {"fi": "a\nb"}}),
            json!({"timezone": "Europe//Helsinki"}),
            json!({"timezone": "Europe/Hel sinki"}),
            json!({"feedContactEmail": "nobody"}),
            json!({"feedContactEmail": "a@b"}),
            json!({"feedContactEmail": "@b.fi"}),
            json!({"feedContactEmail": "a@.fi"}),
            json!({"feedContactEmail": "a@b.fi."}),
            json!({"feedContactEmail": "a@b@c.fi"}),
            json!({"feedContactEmail": "a b@c.fi"}),
            json!({"openingHours": ""}),
            json!({"ttl": 86_401}),
            json!({"colour": "red"}),
        ];
        for extra in bad {
            let refused = System::of(Some(&json!([output(&extra)])), "bikes");
            assert!(refused.is_err(), "{extra}");
        }
        let twice = json!([output(&json!({})), output(&json!({}))]);
        assert!(System::of(Some(&twice), "bikes").is_err());
        let many: Vec<Value> = (0..21)
            .map(|i| output(&json!({"name": format!("b{i}")})))
            .collect();
        assert!(System::of(Some(&json!(many)), "b1").is_err());
        assert!(System::of(Some(&json!({"name": "bikes"})), "bikes").is_err());
        Ok(())
    }

    fn system() -> Result<System, String> {
        let written = json!([output(&json!({"ttl": 30, "systemId": "hsl-citybikes"}))]);
        Ok(System::of(Some(&written), "bikes")?.ok_or("absent")?)
    }

    fn station(id: &str, extra: &Value) -> Value {
        let at = "2026-10-23T10:00:00+02:00";
        let base = json!({
            "id": id, "type": "BikeHireDockingStation",
            "name": {"type": "LanguageProperty", "languageMap": {"fi": "Kamppi", "sv": "Kampen", "x-1": "?"}},
            "location": {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [24.93, 60.17]}},
            "availableBikeNumber": {"type": "Property", "value": 4, "observedAt": at},
            "freeSlotNumber": {"type": "Property", "value": 6, "observedAt": at},
            "totalSlotNumber": {"type": "Property", "value": 10, "observedAt": at},
            "status": {"type": "Property", "value": "working", "observedAt": at},
        });
        with(base, extra)
    }

    #[test]
    fn a_station_maps_to_its_information_and_status() -> Result<(), String> {
        let s = system()?;
        let stations = [station("urn:a", &json!({}))];
        let (info, skipped) = file("station_information", &s, "v", &stations, "u");
        assert_eq!(skipped, 0);
        assert_eq!(
            at(&info, "/data/stations/0"),
            json!({"station_id": "urn:a", "lat": 60.17, "lon": 24.93, "capacity": 10,
                   "name": [{"text": "Kamppi", "language": "fi"}, {"text": "Kampen", "language": "sv"}]})
        );
        let (status, _) = file("station_status", &s, "v", &stations, "u");
        assert_eq!(
            at(&status, "/data/stations/0"),
            json!({"station_id": "urn:a", "num_vehicles_available": 4, "num_docks_available": 6,
                   "is_installed": true, "is_renting": true, "is_returning": true,
                   "last_reported": "2026-10-23T08:00:00Z"})
        );
        assert_eq!(at(&status, "/last_updated"), "2026-10-23T08:00:00Z");
        assert_eq!(
            (at(&status, "/ttl").clone(), at(&status, "/version").clone()),
            (json!(30), json!("3.0"))
        );
        Ok(())
    }

    /// F-ANT5-009: an Attribute whose instances all carry a datasetId is read from the
    /// first one, so the station stays in the feed.
    #[test]
    fn a_named_only_instance_still_reports_the_station() -> Result<(), String> {
        let when = "2026-10-23T10:00:00+02:00";
        let bikes = json!({"availableBikeNumber": [
            {"type": "Property", "datasetId": "urn:ngsi-ld:Dataset:hsl", "value": 4, "observedAt": when}]});
        let stations = [station("urn:a", &bikes)];
        let (status, skipped) = file("station_status", &system()?, "v", &stations, "u");
        assert_eq!(skipped, 0);
        assert_eq!(
            at(&status, "/data/stations/0/num_vehicles_available"),
            json!(4)
        );
        Ok(())
    }

    #[test]
    fn what_a_station_lacks_stays_out_or_leaves_it_out() -> Result<(), String> {
        let s = system()?;
        let plain = json!({"name": {"type": "Property", "value": "Töölö"}, "totalSlotNumber": null,
                           "freeSlotNumber": null, "status": [{"type": "Property", "value": "outOfService"}]});
        let stations = [
            station("urn:plain", &plain),
            station("urn:nameless", &json!({"name": null})),
            station(
                "urn:badname",
                &json!({"name": {"type": "Property", "value": 7}}),
            ),
            station("urn:nowhere", &json!({"location": null})),
            station(
                "urn:line",
                &json!({"location": {"type": "GeoProperty", "value":
                {"type": "LineString", "coordinates": [[24.0, 60.0], [24.1, 60.1]]}}}),
            ),
            station("urn:silent", &json!({"availableBikeNumber": null})),
            station(
                "urn:unreported",
                &json!({"availableBikeNumber": {"type": "Property", "value": 1}}),
            ),
            station(
                "urn:badtime",
                &json!({"availableBikeNumber": {"type": "Property", "value": 1, "observedAt": "soon"}}),
            ),
            station(
                "urn:numtime",
                &json!({"availableBikeNumber": {"type": "Property", "value": 1, "observedAt": 5}}),
            ),
        ];
        let (info, skipped) = file("station_information", &s, "v", &stations, "u");
        assert_eq!(skipped, 4, "{info}");
        let kept = at(&info, "/data/stations");
        assert_eq!(
            at(&kept, "/0/name"),
            json!([{"text": "Töölö", "language": "fi"}])
        );
        assert!(at(&kept, "/0").get("capacity").is_none(), "{kept}");
        let (status, skipped) = file("station_status", &s, "v", &stations, "u");
        assert_eq!(skipped, 4, "{status}");
        let first = at(&status, "/data/stations/0");
        assert_eq!(
            (
                at(&first, "/is_renting").clone(),
                at(&first, "/is_returning").clone()
            ),
            (json!(false), json!(false))
        );
        assert!(first.get("num_docks_available").is_none(), "{first}");
        Ok(())
    }

    #[test]
    fn the_discovery_and_system_files_name_the_feed() -> Result<(), String> {
        let s = system()?;
        let stations = [station("urn:a", &json!({}))];
        let (gbfs, _) = file("gbfs", &s, "v", &stations, "https://x/v/gbfs/v1/bikes");
        assert_eq!(
            at(&gbfs, "/data/feeds/2"),
            json!({"name": "station_status", "url": "https://x/v/gbfs/v1/bikes/station_status.json"})
        );
        let (info, _) = file("system_information", &s, "v", &stations, "u");
        assert_eq!(at(&info, "/data/system_id"), "hsl-citybikes");
        assert_eq!(at(&info, "/data/languages"), json!(["en", "fi", "sv"]));
        assert_eq!(at(&info, "/data/timezone"), "Europe/Helsinki");
        let mut unnamed = system()?;
        unnamed.id = None;
        let (info, _) = file("system_information", &unnamed, "v", &[], "u");
        assert_eq!(at(&info, "/data/system_id"), "v-bikes");
        let stamp = at(&info, "/last_updated");
        assert_eq!(
            stamp.as_str().map(str::len),
            Some("2026-10-23T08:00:00Z".len())
        );
        Ok(())
    }
}
