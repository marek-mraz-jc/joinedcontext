// SPDX-License-Identifier: EUPL-1.2
//! NGSI-LD Entities as the resources of OGC SensorThings API v1.1, Sensing profile, written
//! against the ISO 19156:2023 (OMS) concepts each set stands for (`docs/analysis/sta-mapping.md`).
//!
//! Every function reads one Entity the view answered, under the core @context, and builds the
//! resources it holds; nothing is invented for what the Entity does not carry (EP-13).
//!
//! The ids are a contract (`docs/src/sta.md` section "Resource ids", version 1): an Entity id `E`,
//! an Attribute name `A` as the view answers it and a `datasetId` `D` enter an id through [`part`],
//! an instant `T` through [`instant`], so every id splits back into the parts it was made of.

use antares_ops::outputs::is_uri;
use serde_json::{json, Map, Value};

/// The UN/CEFACT unit code list a `unitCode` is read from.
pub const UNCEFACT: &str = "https://vocabulary.uncefact.org/UnitMeasureCode#";

/// The version of the id grammar this module writes.
pub const ID_GRAMMAR: u32 = 1;

/// The id of the one Sensor of the Datastreams whose Entity names no Device: no Entity id, since
/// every Entity id is an absolute URI and this is none.
pub const UNKNOWN_SENSOR: &str = "unknown";

/// The `observationType` of a measurement with a unit or a fraction.
const MEASUREMENT: &str = "http://www.opengis.net/def/observationType/OGC-OM/2.0/OM_Measurement";

/// The `observationType` of a whole number without a unit: a count.
const COUNT: &str = "http://www.opengis.net/def/observationType/OGC-OM/2.0/OM_CountObservation";

/// The symbols of the UN/CEFACT codes Smart Data Models use most (UCUM spelling).
const SYMBOLS: [(&str, &str); 24] = [
    ("A97", "hPa"),
    ("AMP", "A"),
    ("C62", "1"),
    ("CEL", "°C"),
    ("DD", "°"),
    ("GQ", "µg/m³"),
    ("GRM", "g"),
    ("HTZ", "Hz"),
    ("KGM", "kg"),
    ("KMH", "km/h"),
    ("KMT", "km"),
    ("KWH", "kWh"),
    ("KWT", "kW"),
    ("LTR", "l"),
    ("MMT", "mm"),
    ("MQH", "m³/h"),
    ("MTK", "m²"),
    ("MTQ", "m³"),
    ("MTR", "m"),
    ("MTS", "m/s"),
    ("P1", "%"),
    ("SEC", "s"),
    ("VLT", "V"),
    ("WTT", "W"),
];

/// One resource's self link, its id as an OData key literal (a quote written twice),
/// percent-encoded but for the unreserved characters, `:` and the quote: no id character ends
/// the path (`#`, `?`), adds a segment (`/`, so no `..` a client resolves) or escapes (`%`).
/// The route decodes the path once before it reads the key (T-5013).
#[must_use]
pub fn self_link(base: &str, set: &str, id: &str) -> String {
    let key = antares_ops::percent::encode(&id.replace('\'', "''"))
        .replace("%3A", ":")
        .replace("%27", "'");
    format!("{base}/{set}('{key}')")
}

/// One part of an id: `%`, `/` and `#` percent-encoded, so no part holds a separator and the id
/// splits back exactly (T-5205: an `https:` Entity id or an IRI `datasetId` broke the split).
#[must_use]
pub fn part(raw: &str) -> String {
    raw.replace('%', "%25")
        .replace('/', "%2F")
        .replace('#', "%23")
}

/// A part read back; `None` for one [`part`] cannot have written.
#[must_use]
pub fn unpart(text: &str) -> Option<String> {
    antares_ops::percent::decoded(text).filter(|_| !text.contains(['/', '#']))
}

/// An instant as an id and `phenomenonTime` carry it: a UTC time with exactly three fractional
/// digits (`2026-01-01T00:00:00.000Z`), so `…Z` and `….000Z` from two stores are one id. A time
/// with an offset or no seconds is kept as stored.
#[must_use]
pub fn instant(time: &str) -> String {
    let Some(clock) = time.strip_suffix('Z') else {
        return time.to_owned();
    };
    let (whole, fraction) = clock.split_once('.').unwrap_or((clock, ""));
    if whole.len() != 19 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return time.to_owned();
    }
    let digits: String = fraction.chars().chain("000".chars()).take(3).collect();
    format!("{whole}.{digits}Z")
}

/// The instant one millisecond after `time` ([`instant`] form), for a window that ends just past
/// it; `None` for a time that is not one.
#[must_use]
pub fn next_milli(time: &str) -> Option<String> {
    let t = instant(time);
    let num = |range: std::ops::Range<usize>| t.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second, milli) = (num(11..13)?, num(14..16)?, num(17..19)?, num(20..23)?);
    if !t.ends_with('Z') || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let ms = days
        .checked_mul(86_400_000)?
        .checked_add(hour.checked_mul(3_600_000)?)?
        .checked_add(minute.checked_mul(60_000)?)?
        .checked_add(second.checked_mul(1000)?)?
        .checked_add(milli)?
        .checked_add(1)?;
    let (days, rest) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    let (y, m, d) = civil_from_days(days);
    let unit = |size: i64, of: i64| rest.checked_div(size)?.checked_rem(of);
    Some(format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        unit(3_600_000, 24)?,
        unit(60_000, 60)?,
        unit(1000, 60)?,
        unit(1, 1000)?
    ))
}

/// Days since 1970-01-01 of a proleptic Gregorian date (H. Hinnant's `days_from_civil`); the
/// caller checked a four-digit year, so nothing wraps.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = year.wrapping_sub(i64::from(month <= 2));
    let era = y.div_euclid(400);
    let yoe = y.wrapping_sub(era.wrapping_mul(400));
    let mp = month.wrapping_add(9).wrapping_rem(12);
    let doy = (mp.wrapping_mul(153).wrapping_add(2) / 5).wrapping_add(day.wrapping_sub(1));
    let doe = yoe
        .wrapping_mul(365)
        .wrapping_add(yoe / 4)
        .wrapping_sub(yoe / 100)
        .wrapping_add(doy);
    era.wrapping_mul(146_097)
        .wrapping_add(doe)
        .wrapping_sub(719_468)
}

/// The date of a day count since 1970-01-01 (H. Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days.wrapping_add(719_468);
    let era = z.div_euclid(146_097);
    let doe = z.wrapping_sub(era.wrapping_mul(146_097));
    let yoe = doe
        .wrapping_sub(doe / 1460)
        .wrapping_add(doe / 36_524)
        .wrapping_sub(doe / 146_096)
        / 365;
    let doy = doe.wrapping_sub(
        yoe.wrapping_mul(365)
            .wrapping_add(yoe / 4)
            .wrapping_sub(yoe / 100),
    );
    let mp = doy.wrapping_mul(5).wrapping_add(2) / 153;
    let day = doy
        .wrapping_sub(mp.wrapping_mul(153).wrapping_add(2) / 5)
        .wrapping_add(1);
    let month = if mp < 10 {
        mp.wrapping_add(3)
    } else {
        mp.wrapping_sub(9)
    };
    let year = yoe
        .wrapping_add(era.wrapping_mul(400))
        .wrapping_add(i64::from(month <= 2));
    (year, month, day)
}

/// The last segment of a name an answer spells as an IRI, the name itself otherwise: the term a
/// Smart Data Model and the gateway call the Attribute by.
#[must_use]
pub fn local(name: &str) -> &str {
    name.rsplit(['/', '#']).next().unwrap_or(name)
}

/// One text member of an Entity, a Property's value or the plain string; empty is no text.
fn text<'a>(entity: &'a Value, name: &str) -> Option<&'a str> {
    let member = entity.get(name)?;
    member
        .get("value")
        .unwrap_or(member)
        .as_str()
        .filter(|t| !t.is_empty())
}

/// The Entity's id.
fn id(entity: &Value) -> Option<&str> {
    entity.get("id").and_then(Value::as_str)
}

/// The instances of one Attribute: the array of a multi-instance one, else the one.
#[must_use]
pub fn instances(member: &Value) -> &[Value] {
    match member {
        Value::Array(items) => items,
        one => std::slice::from_ref(one),
    }
}

/// Whether an instance holds a number: a measurement. A text, a geometry, a Relationship and the
/// deletion marker `"urn:ngsi-ld:null"` (NGSI-LD 4.5.7) are none.
fn numeric(instance: &Value) -> bool {
    instance.get("value").is_some_and(Value::is_number)
}

/// The measurements of an Entity (ISO 19156 Observation results): every instance of a Property
/// whose value is a number, with its Attribute's name. A Relationship, a text, a geometry and
/// the Entity's own members carry no numeric `value` and are none.
#[must_use]
pub fn measurements(entity: &Value) -> Vec<(&str, &Value)> {
    entity
        .as_object()
        .into_iter()
        .flatten()
        .flat_map(|(name, member)| instances(member).iter().map(move |i| (name.as_str(), i)))
        .filter(|(_, i)| numeric(i))
        .collect()
}

/// A Datastream's id: `{E}/{A}`, `#{D}` after it for an instance that names a `datasetId`. `A` is
/// the name as the view answers it, an IRI where the core @context has no term, so two
/// vocabularies' `NO2` are two Datastreams.
#[must_use]
pub fn stream_id(urn: &str, name: &str, instance: &Value) -> String {
    match dataset(instance) {
        Some(ds) => format!("{}/{}#{}", part(urn), part(name), part(ds)),
        None => format!("{}/{}", part(urn), part(name)),
    }
}

/// An instance's `datasetId`, when it names one.
#[must_use]
pub fn dataset(instance: &Value) -> Option<&str> {
    instance
        .get("datasetId")
        .and_then(Value::as_str)
        .filter(|d| !d.is_empty())
}

/// The Entity id in front of any id this surface makes from one (`{E}/…`).
#[must_use]
pub fn urn_of(id: &str) -> Option<String> {
    let first = id.split('/').next().unwrap_or(id);
    unpart(first).filter(|e| is_uri(e))
}

/// The Datastream id in front of an Observation id (`{E}/{A}[#{D}]/{T}`).
#[must_use]
pub fn stream_of(observation: &str) -> Option<&str> {
    let (stream, time) = observation.rsplit_once('/')?;
    (!time.is_empty() && split_stream(stream).is_some()).then_some(stream)
}

/// A Datastream id as its Entity id, its Attribute's name and its `datasetId`.
#[must_use]
pub fn split_stream(id: &str) -> Option<(String, String, Option<String>)> {
    let (urn, rest) = id.split_once('/')?;
    let (name, ds) = match rest.split_once('#') {
        Some((name, ds)) => (name, Some(unpart(ds)?)),
        None => (rest, None),
    };
    let (urn, name) = (unpart(urn)?, unpart(name)?);
    (is_uri(&urn) && !name.is_empty()).then_some((urn, name, ds))
}

/// The Entity as a Thing (ISO 19156 Host): `name` and `description` from the members a Smart
/// Data Model keeps them in, the URN for a missing name; every other non-measurement member in
/// `properties`. `expand` inlines `Locations`, `Datastreams` and `Datastreams/Observations`.
#[must_use]
pub fn thing(base: &str, entity: &Value, expand: &[&str]) -> Option<Value> {
    let urn = id(entity)?;
    let link = self_link(base, "Things", urn);
    let mut out = Map::new();
    out.insert("@iot.id".into(), json!(urn));
    out.insert("@iot.selfLink".into(), json!(link));
    out.insert("name".into(), json!(text(entity, "name").unwrap_or(urn)));
    out.insert(
        "description".into(),
        json!(text(entity, "description").unwrap_or_default()),
    );
    out.insert("properties".into(), properties(entity));
    let inner: &[&str] = if expand.contains(&"Datastreams/Observations") {
        &["Observations"]
    } else {
        &[]
    };
    for set in ["Locations", "Datastreams", "HistoricalLocations"] {
        let items = match set {
            "Locations" if expand.contains(&set) => location(base, entity).into_iter().collect(),
            "Datastreams" if expand.iter().any(|e| e.starts_with(set)) => {
                datastreams(base, entity, inner)
            }
            _ => {
                out.insert(
                    format!("{set}@iot.navigationLink"),
                    json!(format!("{link}/{set}")),
                );
                continue;
            }
        };
        out.insert(format!("{set}@iot.count"), json!(items.len()));
        out.insert(set.into(), Value::Array(items));
    }
    Some(Value::Object(out))
}

/// The Entity's type and its members that are no measurement and no `location`, each flattened
/// to its value as [`flat`] reads it.
fn properties(entity: &Value) -> Value {
    let mut out = Map::new();
    for (name, member) in entity.as_object().into_iter().flatten() {
        if matches!(
            name.as_str(),
            "id" | "location" | "@context" | "createdAt" | "modifiedAt"
        ) {
            continue;
        }
        if instances(member).iter().any(numeric) {
            continue;
        }
        out.insert(local(name).to_owned(), flat(member));
    }
    Value::Object(out)
}

/// One member's value as the OGC Features `properties` carry it: the value, object or list, and
/// a language map, JSON or vocabulary wrapped in its member name; never the whole Attribute with
/// its system members (T-5913: a `LanguageProperty` and a `ListProperty` kept `createdAt`).
fn flat(member: &Value) -> Value {
    ["value", "object", "valueList", "objectList"]
        .into_iter()
        .find_map(|k| member.get(k).cloned())
        .or_else(|| {
            ["languageMap", "json", "vocab"]
                .into_iter()
                .find_map(|k| member.get(k).map(|v| json!({ k: v })))
        })
        .unwrap_or_else(|| member.clone())
}

/// The geometry of a GeoProperty instance, when it is one.
fn geometry(member: &Value) -> Option<&Value> {
    let geometry = member.get("value").unwrap_or(member);
    geometry
        .get("coordinates")
        .or_else(|| geometry.get("geometries"))
        .map(|_| geometry)
}

/// A Location of `urn`: id `lid`, its geometry `geometry`.
fn place(base: &str, lid: &str, geometry: &Value, description: &str) -> Value {
    let link = self_link(base, "Locations", lid);
    json!({
        "@iot.id": lid,
        "@iot.selfLink": link,
        "name": "location",
        "description": description,
        "encodingType": "application/geo+json",
        "location": geometry,
        "Things@iot.navigationLink": format!("{link}/Things"),
        "HistoricalLocations@iot.navigationLink": format!("{link}/HistoricalLocations"),
    })
}

/// The Entity's current `location` as a Location (ISO 19156 Host's spatial position), id
/// `{E}/location`. Only a single-instance `location` is one (`docs/src/sta.md`, not served).
#[must_use]
pub fn location(base: &str, entity: &Value) -> Option<Value> {
    let urn = id(entity)?;
    let geometry = geometry(entity.get("location")?)?;
    let lid = format!("{}/location", part(urn));
    Some(place(
        base,
        &lid,
        geometry,
        "The entity's primary GeoProperty",
    ))
}

/// The Location an Entity's `location` history held at `time`, id `{E}/location/{T}`.
#[must_use]
pub fn location_at(base: &str, history: &Value, time: &str) -> Option<Value> {
    let urn = id(history)?;
    let instance = history
        .get("location")
        .map(instances)
        .unwrap_or_default()
        .iter()
        .find(|i| {
            i.get("observedAt").and_then(Value::as_str).map(instant) == Some(instant(time))
        })?;
    let lid = format!("{}/location/{}", part(urn), instant(time));
    Some(place(
        base,
        &lid,
        geometry(instance)?,
        "The entity's primary GeoProperty at this time",
    ))
}

/// A Datastream's `unitOfMeasurement` (mandatory in STA 1.1): from the instance's `unitCode`, its
/// symbol where the code is a common one, else every member null (nothing is invented, EP-13).
fn unit(instance: &Value) -> Value {
    let Some(code) = instance.get("unitCode").and_then(Value::as_str) else {
        return json!({ "name": null, "symbol": null, "definition": null });
    };
    let symbol = SYMBOLS
        .iter()
        .find(|(c, _)| *c == code)
        .map_or(code, |(_, s)| s);
    json!({ "name": code, "symbol": symbol, "definition": format!("{UNCEFACT}{code}") })
}

/// The `observationType` of an instance: a whole number without a unit is a count.
fn kind(instance: &Value) -> &'static str {
    let whole = instance
        .get("value")
        .is_some_and(|v| v.is_i64() || v.is_u64());
    if whole && instance.get("unitCode").is_none() {
        COUNT
    } else {
        MEASUREMENT
    }
}

/// One Datastream (ISO 19156 ObservationGroup) per measured instance (rules 2 and 6).
/// `expand` naming `Observations` inlines the current one.
#[must_use]
pub fn datastreams(base: &str, entity: &Value, expand: &[&str]) -> Vec<Value> {
    let Some(urn) = id(entity) else {
        return Vec::new();
    };
    let inline = expand.contains(&"Observations");
    measurements(entity)
        .into_iter()
        .map(|(name, instance)| {
            let sid = stream_id(urn, name, instance);
            let link = self_link(base, "Datastreams", &sid);
            let mut out = Map::new();
            out.insert("@iot.id".into(), json!(sid));
            out.insert("@iot.selfLink".into(), json!(link));
            out.insert("name".into(), json!(local(name)));
            out.insert(
                "description".into(),
                json!(format!("{} of {urn}", local(name))),
            );
            out.insert("observationType".into(), json!(kind(instance)));
            out.insert("unitOfMeasurement".into(), unit(instance));
            out.insert(
                "Observations@iot.navigationLink".into(),
                json!(format!("{link}/Observations")),
            );
            if inline {
                let current: Vec<Value> = observation(base, &sid, instance, times(entity))
                    .into_iter()
                    .collect();
                out.insert("Observations".into(), Value::Array(current));
            }
            out.insert(
                "Thing@iot.navigationLink".into(),
                json!(self_link(base, "Things", urn)),
            );
            out.insert(
                "ObservedProperty@iot.navigationLink".into(),
                json!(format!("{link}/ObservedProperty")),
            );
            // Exactly one Sensor (STA 1.1): the Device the Entity names, else the unknown one.
            out.insert(
                "Sensor@iot.navigationLink".into(),
                json!(format!("{link}/Sensor")),
            );
            Value::Object(out)
        })
        .collect()
}

/// The Entity's own `observedAt` and `modifiedAt`, the times an instance without its own falls
/// back to.
fn times(entity: &Value) -> [Option<&str>; 2] {
    ["observedAt", "modifiedAt"].map(|t| entity.get(t).and_then(Value::as_str))
}

/// One Observation per measured instance that has a time: the current one each holds.
#[must_use]
pub fn observations(base: &str, entity: &Value) -> Vec<Value> {
    let Some(urn) = id(entity) else {
        return Vec::new();
    };
    measurements(entity)
        .into_iter()
        .filter_map(|(name, instance)| {
            observation(
                base,
                &stream_id(urn, name, instance),
                instance,
                times(entity),
            )
        })
        .collect()
}

/// The Observations of one Datastream from the temporal answer of its Entity: the numeric
/// instances of `attribute` with the Datastream's `datasetId` (none for the default one).
#[must_use]
pub fn series(base: &str, entity: &Value, attribute: &str, stream: &str) -> Vec<Value> {
    let ds = split_stream(stream).and_then(|(_, _, ds)| ds);
    entity
        .get(attribute)
        .map(instances)
        .unwrap_or_default()
        .iter()
        .filter(|i| numeric(i) && dataset(i) == ds.as_deref())
        .filter_map(|i| observation(base, stream, i, times(entity)))
        .collect()
}

/// One instance as an Observation (ISO 19156 Observation): id `{datastream id}/{T}`. STA 1.1
/// makes `phenomenonTime` mandatory: the instance's `observedAt`, else the Entity's, else when the
/// value was last set (the instance's `modifiedAt`, else the Entity's), so a value of a type no
/// sensor measures still has its time; `parameters.timeSource` then says so and `resultTime` is
/// null. An instance with none of them is no Observation (EP-13: no time is invented).
fn observation(
    base: &str,
    stream: &str,
    instance: &Value,
    [observed, modified]: [Option<&str>; 2],
) -> Option<Value> {
    let own = |t: &str| instance.get(t).and_then(Value::as_str);
    let sensed = own("observedAt").or(observed).map(instant);
    let at = match &sensed {
        Some(t) => t.clone(),
        None => instant(own("modifiedAt").or(modified)?),
    };
    let oid = format!("{stream}/{at}");
    let link = self_link(base, "Observations", &oid);
    let mut out = Map::new();
    out.insert("@iot.id".into(), json!(oid));
    out.insert("@iot.selfLink".into(), json!(link));
    out.insert(
        "result".into(),
        instance.get("value").cloned().unwrap_or_default(),
    );
    out.insert("phenomenonTime".into(), json!(at));
    // Mandatory, null allowed (STA 1.1 table 11): when a sensor observed the result, else null.
    out.insert("resultTime".into(), json!(sensed));
    if sensed.is_none() {
        out.insert("parameters".into(), json!({ "timeSource": "modifiedAt" }));
    }
    out.insert(
        "Datastream@iot.navigationLink".into(),
        json!(self_link(base, "Datastreams", stream)),
    );
    out.insert(
        "FeatureOfInterest@iot.navigationLink".into(),
        json!(format!("{link}/FeatureOfInterest")),
    );
    Some(Value::Object(out))
}

/// One ObservedProperty (ISO 19156 ObservableProperty) per measured Attribute (rule 1): id and
/// `definition` its expanded IRI, so one Attribute measured anywhere is one ObservedProperty.
#[must_use]
pub fn observed_properties(base: &str, entity: &Value) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for (name, _) in measurements(entity) {
        let iri = antares_ops::core_iri(name);
        if out.iter().any(|p| p.get("@iot.id") == Some(&json!(iri))) {
            continue;
        }
        let link = self_link(base, "ObservedProperties", &iri);
        out.push(json!({
            "@iot.id": iri,
            "@iot.selfLink": link,
            "Datastreams@iot.navigationLink": format!("{link}/Datastreams"),
            "name": local(name),
            "definition": iri,
            "description": format!("Observed property {}", local(name)),
        }));
    }
    out
}

/// The object of one Relationship of the Entity.
#[must_use]
pub fn object<'a>(entity: &'a Value, name: &str) -> Option<&'a str> {
    entity.get(name)?.get("object")?.as_str()
}

/// An SDM Device as a Sensor (ISO 19156 Observer, rule 3): `metadata` its `controlledProperty`,
/// encoded as JSON (the profile `docs/src/sta.md` names). An Entity of another type is no Sensor.
#[must_use]
pub fn sensor(base: &str, device: &Value) -> Option<Value> {
    let did = id(device)?;
    let kind = device.get("type").and_then(Value::as_str)?;
    if local(kind) != "Device" {
        return None;
    }
    let metadata = device
        .get("controlledProperty")
        .map(|m| m.get("value").unwrap_or(m));
    let link = self_link(base, "Sensors", did);
    Some(json!({
        "@iot.id": did,
        "@iot.selfLink": link,
        "Datastreams@iot.navigationLink": format!("{link}/Datastreams"),
        "name": text(device, "name").unwrap_or(did),
        "description": text(device, "description").unwrap_or_default(),
        "encodingType": "application/json",
        "metadata": metadata.cloned().unwrap_or_else(|| json!([])),
    }))
}

/// The Sensor of every Datastream whose Entity names no Device (STA 1.1: exactly one Sensor per
/// Datastream), id [`UNKNOWN_SENSOR`]: it says no more than that the view names none.
#[must_use]
pub fn unknown_sensor(base: &str) -> Value {
    let link = self_link(base, "Sensors", UNKNOWN_SENSOR);
    json!({
        "@iot.id": UNKNOWN_SENSOR,
        "@iot.selfLink": link,
        "Datastreams@iot.navigationLink": format!("{link}/Datastreams"),
        "name": "Unknown sensor",
        "description": "The view names no Device for these Datastreams.",
        "encodingType": "application/json",
        "metadata": [],
    })
}

/// A FeatureOfInterest (rule 4): the target of `refPointOfInterest` with its own URN and
/// location, or for an Entity without one its own `location`, id `{E}/location`. Without a
/// geometry there is none.
#[must_use]
pub fn feature(base: &str, entity: &Value, fid: &str) -> Option<Value> {
    let geometry = geometry(entity.get("location")?)?;
    let link = self_link(base, "FeaturesOfInterest", fid);
    Some(json!({
        "@iot.id": fid,
        "@iot.selfLink": link,
        "Observations@iot.navigationLink": format!("{link}/Observations"),
        "name": text(entity, "name").unwrap_or(fid),
        "description": text(entity, "description").unwrap_or_default(),
        "encodingType": "application/geo+json",
        "feature": geometry,
    }))
}

/// The HistoricalLocations of an Entity from the temporal answer of its `location` (rule 5):
/// one per instance with an `observedAt`, id `{E}/historicalLocation/{T}`, its Locations the
/// geometry of that time (`{E}/location/{T}`).
#[must_use]
pub fn historical_locations(base: &str, entity: &Value) -> Vec<Value> {
    let Some(urn) = id(entity) else {
        return Vec::new();
    };
    entity
        .get("location")
        .map(instances)
        .unwrap_or_default()
        .iter()
        .filter_map(|i| i.get("observedAt").and_then(Value::as_str))
        .map(|time| {
            let time = instant(time);
            let hid = format!("{}/historicalLocation/{time}", part(urn));
            let link = self_link(base, "HistoricalLocations", &hid);
            json!({
                "@iot.id": hid,
                "Locations@iot.navigationLink": format!("{link}/Locations"),
                "@iot.selfLink": link,
                "time": time,
                "Thing@iot.navigationLink": self_link(base, "Things", urn),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(v: &Value, path: &str) -> Value {
        v.pointer(path).cloned().unwrap_or_default()
    }

    fn ids(items: &[Value]) -> Vec<String> {
        items
            .iter()
            .map(|i| at(i, "/@iot.id").as_str().unwrap_or_default().to_owned())
            .collect()
    }

    const BASE: &str = "http://h/x/views/v/sta/v1.1";

    const SDM_NO2: &str =
        "urn:ngsi-ld:Car:1/https:%2F%2Fsmartdatamodels.org%2FdataModel.Environment%2FNO2";

    fn car() -> Value {
        json!({
            "id": "urn:ngsi-ld:Car:1", "type": "Car",
            "name": {"type": "Property", "value": "Žltá"},
            "location": {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [17.1, 48.1]}},
            "speed": {"type": "Property", "value": 42, "unitCode": "KMH", "observedAt": "2026-01-01T00:00:00Z"},
            "https://smartdatamodels.org/dataModel.Environment/NO2": [
                {"type": "Property", "value": 15.0, "datasetId": "urn:ngsi-ld:Dataset:a"},
                {"type": "Property", "value": 16.0}
            ],
            "colour": {"type": "Property", "value": "red"},
            "at": {"type": "Relationship", "object": "urn:ngsi-ld:Stop:1"}
        })
    }

    #[test]
    fn a_thing_property_is_the_value_of_every_kind_never_the_attribute() {
        let at = "2026-10-01T10:00:00Z";
        let e = json!({"id": "urn:ngsi-ld:Car:9", "type": "Car",
            "label": {"type": "LanguageProperty", "languageMap": {"sk": "auto"}, "createdAt": at},
            "tags": {"type": "ListProperty", "valueList": ["a"], "createdAt": at},
            "near": {"type": "ListRelationship", "objectList": [{"object": "urn:ngsi-ld:Stop:1"}]},
            "spec": {"type": "JsonProperty", "json": {"k": 1}},
            "state": {"type": "VocabProperty", "vocab": "parked"},
            "owner": {"type": "Relationship", "object": "urn:ngsi-ld:Person:1"},
            "colour": {"type": "Property", "value": "red"},
            "odd": [{"type": "Property", "value": "x"}]});
        assert_eq!(
            properties(&e),
            json!({"type": "Car", "label": {"languageMap": {"sk": "auto"}}, "tags": ["a"],
                   "near": [{"object": "urn:ngsi-ld:Stop:1"}], "spec": {"json": {"k": 1}},
                   "state": {"vocab": "parked"}, "owner": "urn:ngsi-ld:Person:1",
                   "colour": "red", "odd": [{"type": "Property", "value": "x"}]})
        );
    }

    #[test]
    fn a_thing_carries_its_name_its_properties_and_links_or_inlines_its_sets() {
        let t = thing(BASE, &car(), &[]).unwrap_or_default();
        assert_eq!(at(&t, "/@iot.id"), "urn:ngsi-ld:Car:1");
        assert_eq!(
            at(&t, "/@iot.selfLink"),
            format!("{BASE}/Things('urn:ngsi-ld:Car:1')")
        );
        assert_eq!(at(&t, "/name"), "Žltá");
        assert_eq!(at(&t, "/description"), "");
        assert_eq!(
            at(&t, "/properties"),
            json!({"type": "Car", "name": "Žltá", "colour": "red", "at": "urn:ngsi-ld:Stop:1"})
        );
        assert_eq!(
            at(&t, "/Datastreams@iot.navigationLink"),
            format!("{BASE}/Things('urn:ngsi-ld:Car:1')/Datastreams")
        );
        let t = thing(BASE, &car(), &["Locations", "Datastreams"]).unwrap_or_default();
        assert_eq!(at(&t, "/Locations@iot.count"), 1);
        assert_eq!(at(&t, "/Datastreams@iot.count"), 3);
        assert_eq!(at(&t, "/Datastreams/0/Observations"), Value::Null);
        assert!(t.get("HistoricalLocations@iot.navigationLink").is_some());
        // T-5205 finding 18: depth 2 inlines each Datastream's current Observation.
        let deep = thing(BASE, &car(), &["Datastreams/Observations"]).unwrap_or_default();
        assert_eq!(at(&deep, "/Datastreams/2/Observations/0/result"), 42);
        let bare = thing(BASE, &json!({"id": "urn:a"}), &[]).unwrap_or_default();
        assert_eq!(at(&bare, "/name"), "urn:a");
        assert_eq!(thing(BASE, &json!({}), &[]), None);
    }

    /// T-5205 findings 5, 6, 7 and 21.
    #[test]
    fn each_dataset_of_an_attribute_is_its_own_datastream_named_by_its_full_name() {
        let ds = datastreams(BASE, &car(), &[]);
        assert_eq!(
            ids(&ds),
            [
                format!("{SDM_NO2}#urn:ngsi-ld:Dataset:a"),
                SDM_NO2.to_owned(),
                "urn:ngsi-ld:Car:1/speed".to_owned()
            ]
        );
        let speed = ds.get(2).cloned().unwrap_or_default();
        assert_eq!(
            at(&speed, "/unitOfMeasurement"),
            json!({"name": "KMH", "symbol": "km/h", "definition": format!("{UNCEFACT}KMH")})
        );
        assert_eq!(at(&speed, "/observationType"), MEASUREMENT);
        assert_eq!(
            at(&speed, "/Sensor@iot.navigationLink"),
            format!("{BASE}/Datastreams('urn:ngsi-ld:Car:1%2Fspeed')/Sensor")
        );
        let no2 = ds.first().cloned().unwrap_or_default();
        assert_eq!(at(&no2, "/name"), "NO2");
        assert_eq!(
            at(&no2, "/unitOfMeasurement"),
            json!({"name": null, "symbol": null, "definition": null})
        );
        let other = Value::Array(datastreams(
            BASE,
            &json!({"id": "urn:a", "id2": {"value": 1}, "t": {"value": 2, "unitCode": "XYZ"}}),
            &[],
        ));
        assert_eq!(at(&other, "/0/observationType"), COUNT);
        assert_eq!(at(&other, "/1/observationType"), MEASUREMENT);
        assert_eq!(at(&other, "/1/unitOfMeasurement/symbol"), "XYZ");
        assert_eq!(
            datastreams(BASE, &json!({"speed": {"value": 1}}), &[]).len(),
            0
        );
        let two = json!({"id": "urn:a", "https://a.org/NO2": {"value": 1}, "https://b.org/NO2": {"value": 2}});
        let streams = ids(&datastreams(BASE, &two, &[]));
        assert_eq!(streams.len(), 2);
        assert_ne!(streams.first(), streams.get(1));
    }

    /// T-5205 finding 20: no sensor time, `resultTime` null and the source named.
    #[test]
    fn observations_follow_their_datastream_and_fall_back_to_the_entitys_time() {
        let e = json!({
            "id": "urn:ngsi-ld:Car:1", "observedAt": "2026-02-02T00:00:00Z",
            "speed": {"value": 1, "modifiedAt": "2026-03-03T00:00:00Z"},
            "NO2": {"value": 2, "datasetId": "urn:ngsi-ld:Dataset:a"}
        });
        let obs = observations(BASE, &e);
        assert_eq!(
            ids(&obs),
            [
                "urn:ngsi-ld:Car:1/NO2#urn:ngsi-ld:Dataset:a/2026-02-02T00:00:00.000Z",
                "urn:ngsi-ld:Car:1/speed/2026-02-02T00:00:00.000Z"
            ]
        );
        assert_eq!(
            at(&obs.get(1).cloned().unwrap_or_default(), "/resultTime"),
            "2026-02-02T00:00:00.000Z"
        );
        assert_eq!(
            at(&obs.get(1).cloned().unwrap_or_default(), "/parameters"),
            Value::Null
        );
        let set = observations(
            BASE,
            &json!({"id": "urn:a", "modifiedAt": "2026-04-04T00:00:00Z",
                    "x": {"value": 2}, "y": {"value": 3, "modifiedAt": "2026-05-05T00:00:00Z"}}),
        );
        let times: Vec<(Value, Value, Value)> = set
            .iter()
            .map(|o| {
                (
                    at(o, "/@iot.id"),
                    at(o, "/resultTime"),
                    at(o, "/parameters/timeSource"),
                )
            })
            .collect();
        assert_eq!(
            times,
            [
                (
                    json!("urn:a/x/2026-04-04T00:00:00.000Z"),
                    Value::Null,
                    json!("modifiedAt")
                ),
                (
                    json!("urn:a/y/2026-05-05T00:00:00.000Z"),
                    Value::Null,
                    json!("modifiedAt")
                ),
            ]
        );
        let timeless = observations(BASE, &json!({"id": "urn:a", "x": {"value": 2}}));
        assert_eq!(timeless.len(), 0);
        assert_eq!(observations(BASE, &json!({"x": {"value": 2}})).len(), 0);
    }

    #[test]
    fn a_datastream_inlines_its_current_observation_when_expanded() {
        let ds = datastreams(BASE, &car(), &["Observations"]);
        let speed = ds.get(2).cloned().unwrap_or_default();
        assert_eq!(
            at(&speed, "/Observations").as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(at(&speed, "/Observations/0/result"), 42);
        assert!(datastreams(BASE, &car(), &[])
            .iter()
            .all(|d| d.get("Observations").is_none()));
    }

    /// T-5205 finding 22: a text and the deletion marker are no Observation.
    #[test]
    fn a_series_reads_the_numeric_instances_of_its_own_dataset_only() {
        let history = json!({"id": "urn:ngsi-ld:Car:1", "NO2": [
            {"value": 1, "observedAt": "2026-01-01T00:00:00Z"},
            {"value": "urn:ngsi-ld:null", "observedAt": "2026-01-01T12:00:00Z"},
            {"value": 3, "observedAt": "2026-01-01T18:00:00Z"},
            {"value": 2, "observedAt": "2026-01-02T00:00:00Z", "datasetId": "d"},
            {"observedAt": "2026-01-03T00:00:00Z"}
        ]});
        let default = series(BASE, &history, "NO2", "urn:ngsi-ld:Car:1/NO2");
        assert_eq!(default.len(), 2);
        let d = series(BASE, &history, "NO2", "urn:ngsi-ld:Car:1/NO2#d");
        assert_eq!(
            ids(&d),
            ["urn:ngsi-ld:Car:1/NO2#d/2026-01-02T00:00:00.000Z"]
        );
        assert_eq!(
            series(BASE, &history, "speed", "urn:ngsi-ld:Car:1/speed").len(),
            0
        );
        let timeless = json!({"id": "urn:a", "x": {"value": 3}});
        assert_eq!(series(BASE, &timeless, "x", "urn:a/x").len(), 0);
        let set = json!({"id": "urn:a", "x": {"value": 3, "modifiedAt": "2026-01-04T00:00:00Z"}});
        assert_eq!(series(BASE, &set, "x", "urn:a/x").len(), 1);
    }

    #[test]
    fn one_attribute_is_one_observed_property_by_its_expanded_iri() {
        let ops = observed_properties(BASE, &car());
        assert_eq!(
            ids(&ops),
            [
                "https://smartdatamodels.org/dataModel.Environment/NO2".to_owned(),
                antares_ops::core_iri("speed")
            ]
        );
        assert_eq!(ops.first().map(|p| at(p, "/name")), Some(json!("NO2")));
    }

    #[test]
    fn a_sensor_is_a_device_or_the_unknown_one() {
        let dev = json!({"id": "urn:ngsi-ld:Device:1", "type": "Device",
            "controlledProperty": {"type": "Property", "value": ["speed"]}, "description": "radar"});
        let s = sensor(BASE, &dev).unwrap_or_default();
        assert_eq!(at(&s, "/metadata"), json!(["speed"]));
        assert_eq!(at(&s, "/name"), "urn:ngsi-ld:Device:1");
        assert_eq!(at(&s, "/description"), "radar");
        let plain = sensor(
            BASE,
            &json!({"id": "urn:d", "type": "https://smartdatamodels.org/dataModel.Device/Device"}),
        );
        assert_eq!(plain.map(|s| at(&s, "/metadata")), Some(json!([])));
        assert_eq!(sensor(BASE, &car()), None);
        assert_eq!(sensor(BASE, &json!({"id": "urn:d"})), None);
        assert_eq!(sensor(BASE, &json!({"type": "Device"})), None);
        let unknown = unknown_sensor(BASE);
        assert_eq!(at(&unknown, "/@iot.id"), UNKNOWN_SENSOR);
        assert!(!is_uri(UNKNOWN_SENSOR));
    }

    #[test]
    fn a_location_and_a_feature_need_a_geometry() {
        let l = location(BASE, &car()).unwrap_or_default();
        assert_eq!(at(&l, "/@iot.id"), "urn:ngsi-ld:Car:1/location");
        assert_eq!(
            location(BASE, &json!({"id": "urn:a", "location": {"value": "here"}})),
            None
        );
        assert_eq!(location(BASE, &json!({"id": "urn:a"})), None);
        assert_eq!(
            location(
                BASE,
                &json!({"location": {"value": {"coordinates": [1, 2]}}})
            ),
            None
        );
        let multi =
            json!({"id": "urn:a", "location": {"type": "GeometryCollection", "geometries": []}});
        assert!(location(BASE, &multi).is_some());
        let f = feature(BASE, &car(), "urn:ngsi-ld:Car:1/location").unwrap_or_default();
        assert_eq!(at(&f, "/name"), "Žltá");
        assert_eq!(at(&f, "/feature/type"), "Point");
        assert_eq!(
            feature(BASE, &json!({"id": "urn:a"}), "urn:a/location"),
            None
        );
    }

    /// T-5205 finding 13: each step is its own geometry.
    #[test]
    fn a_location_history_is_one_historical_location_and_one_location_per_timed_instance() {
        let moving = json!({"id": "urn:ngsi-ld:Vehicle:1", "location": [
            {"value": {"type": "Point", "coordinates": [1, 2]}, "observedAt": "2026-01-01T00:00:00Z"},
            {"value": {"type": "Point", "coordinates": [1, 3]}}
        ]});
        let hl = historical_locations(BASE, &moving);
        assert_eq!(
            ids(&hl),
            ["urn:ngsi-ld:Vehicle:1/historicalLocation/2026-01-01T00:00:00.000Z"]
        );
        let then = location_at(BASE, &moving, "2026-01-01T00:00:00.000Z").unwrap_or_default();
        assert_eq!(
            at(&then, "/@iot.id"),
            "urn:ngsi-ld:Vehicle:1/location/2026-01-01T00:00:00.000Z"
        );
        assert_eq!(at(&then, "/location/coordinates"), json!([1, 2]));
        assert_eq!(location_at(BASE, &moving, "2026-01-02T00:00:00Z"), None);
        assert_eq!(location_at(BASE, &json!({"location": []}), "x"), None);
        assert_eq!(historical_locations(BASE, &json!({"id": "urn:a"})).len(), 0);
        assert_eq!(
            historical_locations(BASE, &json!({"location": []})).len(),
            0
        );
    }

    /// T-5205 finding 4: an id with `/`, `#` or `%` in any part splits back into its parts.
    #[test]
    fn ids_split_back_into_their_parts() {
        for (urn, name, ds) in [
            ("urn:a", "speed", None),
            ("https://example.org/e/1", "speed", Some("https://x/ds#1")),
            ("urn:ngsi-ld:Road:a/b", "https://a.org/v#NO2", Some("urn:d")),
            ("urn:a%2F", "x%", None),
        ] {
            let instance = json!({"value": 1, "datasetId": ds});
            let sid = stream_id(urn, name, &instance);
            assert_eq!(
                split_stream(&sid),
                Some((urn.to_owned(), name.to_owned(), ds.map(str::to_owned))),
                "{sid}"
            );
            let oid = format!("{sid}/2026-01-01T00:00:00.000Z");
            assert_eq!(stream_of(&oid), Some(sid.as_str()));
            assert_eq!(urn_of(&oid).as_deref(), Some(urn));
        }
        for refused in [
            "x/speed",
            "urn:a/",
            "urn:a/b/c",
            "urn:a",
            "urn:a/b#c/d",
            "urn:a/%zz",
            "urn:a/b#%zz",
        ] {
            assert_eq!(split_stream(refused), None, "{refused}");
        }
        assert_eq!(stream_of("urn:a/"), None);
        assert_eq!(stream_of("urn:a"), None);
        assert_eq!(urn_of("urn:a"), Some("urn:a".to_owned()));
        assert_eq!(urn_of("x/y"), None);
        assert_eq!(
            self_link(BASE, "Things", "urn:a'b"),
            format!("{BASE}/Things('urn:a''b')")
        );
        assert_eq!(object(&car(), "at"), Some("urn:ngsi-ld:Stop:1"));
        assert_eq!(object(&car(), "home"), None);
        assert!(is_uri("https://example.org/a"));
        assert!(!is_uri("9223372036854775807"));
        assert!(!is_uri(":x"));
        assert!(!is_uri("1a:x"));
        assert!(!is_uri("a b:x"));
    }

    /// T-5205 findings 12 and 25: one spelling per instant, and the instant after it.
    #[test]
    fn instants_are_one_spelling_and_their_next_millisecond_is_exact() {
        for (raw, want) in [
            ("2026-01-01T00:00:00Z", "2026-01-01T00:00:00.000Z"),
            ("2026-01-01T00:00:00.5Z", "2026-01-01T00:00:00.500Z"),
            ("2026-01-01T00:00:00.123456Z", "2026-01-01T00:00:00.123Z"),
            ("2026-01-01T00:00:00+01:00", "2026-01-01T00:00:00+01:00"),
            ("2026-01-01T00:00Z", "2026-01-01T00:00Z"),
            ("2026-01-01T00:00:00.x1Z", "2026-01-01T00:00:00.x1Z"),
        ] {
            assert_eq!(instant(raw), want, "{raw}");
        }
        for (raw, want) in [
            ("2026-01-01T00:00:00Z", Some("2026-01-01T00:00:00.001Z")),
            ("2025-12-31T23:59:59.999Z", Some("2026-01-01T00:00:00.000Z")),
            ("2024-02-28T23:59:59.999Z", Some("2024-02-29T00:00:00.000Z")),
            ("1969-12-31T23:59:59.999Z", Some("1970-01-01T00:00:00.000Z")),
            ("2026-10-22T10:20:30.400Z", Some("2026-10-22T10:20:30.401Z")),
            ("2026-01-01T00:00:00.000+0100", None),
            ("2026-13-01T00:00:00Z", None),
            ("2026-00-01T00:00:00Z", None),
            ("2026-01-32T00:00:00Z", None),
            ("2026-01-00T00:00:00Z", None),
            ("yesterday", None),
        ] {
            assert_eq!(next_milli(raw).as_deref(), want, "{raw}");
        }
    }

    /// T-5205 finding 25: the id grammar is a contract (`docs/src/sta.md`, "Resource ids"). A
    /// change here breaks every client that stored an id: bump [`ID_GRAMMAR`] and say so there.
    #[test]
    fn the_ids_of_a_fixture_are_pinned() {
        assert_eq!(ID_GRAMMAR, 1);
        let road = json!({"id": "https://example.org/road/1", "type": "Road",
            "location": {"value": {"type": "Point", "coordinates": [1, 2]}},
            "https://a.org/v#NO2": {"value": 1.5, "datasetId": "urn:ds:1", "observedAt": "2026-01-01T00:00:00Z"},
            "count": {"value": 3, "modifiedAt": "2026-01-02T00:00:00.5Z"}});
        let moved = json!({"id": "https://example.org/road/1", "location": [
            {"value": {"type": "Point", "coordinates": [1, 2]}, "observedAt": "2026-01-01T00:00:00Z"}]});
        let mut all = vec![at(&thing(BASE, &road, &[]).unwrap_or_default(), "/@iot.id")];
        all.extend(location(BASE, &road).map(|l| at(&l, "/@iot.id")));
        all.extend(location_at(BASE, &moved, "2026-01-01T00:00:00Z").map(|l| at(&l, "/@iot.id")));
        for set in [
            datastreams(BASE, &road, &[]),
            observations(BASE, &road),
            observed_properties(BASE, &road),
            historical_locations(BASE, &moved),
        ] {
            all.extend(set.iter().map(|i| at(i, "/@iot.id")));
        }
        assert_eq!(
            all,
            [
                "https://example.org/road/1",
                "https:%2F%2Fexample.org%2Froad%2F1/location",
                "https:%2F%2Fexample.org%2Froad%2F1/location/2026-01-01T00:00:00.000Z",
                "https:%2F%2Fexample.org%2Froad%2F1/count",
                "https:%2F%2Fexample.org%2Froad%2F1/https:%2F%2Fa.org%2Fv%23NO2#urn:ds:1",
                "https:%2F%2Fexample.org%2Froad%2F1/count/2026-01-02T00:00:00.500Z",
                "https:%2F%2Fexample.org%2Froad%2F1/https:%2F%2Fa.org%2Fv%23NO2#urn:ds:1/2026-01-01T00:00:00.000Z",
                antares_ops::core_iri("count").as_str(),
                "https://a.org/v#NO2",
                "https:%2F%2Fexample.org%2Froad%2F1/historicalLocation/2026-01-01T00:00:00.000Z",
            ]
        );
    }
}
