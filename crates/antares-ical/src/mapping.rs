// SPDX-License-Identifier: EUPL-1.2
//! One Entity as one RFC 5545 `VEVENT` through the view's field mapping
//! (`docs/analysis/ical-mapping.md`, platform API/02 §8b, T-4901).
//!
//! A mapping names, for each calendar field, the NGSI-LD attribute it comes from and at most the
//! remaps of a fixed allow-list (`firstOf`, `join`, `constant`, `language`, `date`, `timezone`,
//! `endInclusive`, `map`, `truncate`): data, never an expression. `UID` is the entity id, fixed.
//! `DTSTAMP` is the last revision of what the Event shows: the mapped `LAST-MODIFIED`, else the
//! latest `modifiedAt` of the mapped attributes, else the Entity's (T-5208), so a re-ingest that
//! rewrites other members leaves the feed byte for byte as it was. A time without an offset is
//! read in the field's `timezone`, else the output's (default `Europe/Bratislava`), and written
//! in UTC: no floating time leaves the feed. No mapping is the `sdm-event` preset: `startDate`,
//! `endDate` (the last day, so a date `DTEND` is the day after it), `name`, `description`,
//! `address`, `location`, `url` and `organizer` (`{name, url}` as `ORGANIZER;CN=`). An Entity
//! with no value for `DTSTART` or `SUMMARY`, a date it cannot read, a `DTEND` before
//! `DTSTART` (a date one not after it; an instant end equal to an instant start is written
//! without `DTEND`), a `GEO` off the globe or a `URL` that is not
//! http(s) is no `VEVENT` at all, and its reason is counted (EP-126).

use chrono::{DateTime, Days, NaiveDate, NaiveDateTime, NaiveTime, TimeZone as _, Utc};
use chrono_tz::Tz;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// The last segment of an expanded IRI, or the term itself.
#[must_use]
pub fn local(name: &str) -> &str {
    name.rsplit(['/', '#']).next().unwrap_or(name)
}

/// A point or a day of a calendar: a date, a UTC instant or a time without zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    /// A whole day (`2026-10-20`): `VALUE=DATE`.
    Date(NaiveDate),
    /// An instant with an offset, kept in UTC (`2026-10-20T18:00:00+02:00`).
    At(DateTime<Utc>),
    /// A wall-clock time without an offset (`2026-10-20T18:00:00`): a floating time.
    Floating(NaiveDateTime),
}

impl When {
    /// An RFC 3339 date-time, a date-time without offset or a date; anything else is `None`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if let Ok(t) = DateTime::parse_from_rfc3339(text) {
            return Some(Self::At(t.with_timezone(&Utc)));
        }
        if let Ok(t) = NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f") {
            return Some(Self::Floating(t));
        }
        NaiveDate::parse_from_str(text, "%Y-%m-%d")
            .ok()
            .map(Self::Date)
    }

    /// Where it starts as a UTC instant, a date and a floating time read in `zone`.
    #[must_use]
    pub fn utc_key(self, zone: Tz) -> NaiveDateTime {
        let wall = match self {
            Self::At(t) => return t.naive_utc(),
            Self::Date(d) => d.and_time(NaiveTime::MIN),
            Self::Floating(t) => t,
        };
        zone.from_local_datetime(&wall)
            .earliest()
            .map_or(wall, |t| t.naive_utc())
    }

    /// Just past what it covers as a UTC instant: a date's next midnight in `zone`.
    #[must_use]
    pub fn utc_past(self, zone: Tz) -> NaiveDateTime {
        match self {
            Self::Date(d) => Self::Date(next_day(d)).utc_key(zone),
            other => other.utc_key(zone),
        }
    }

    /// `end` written in the value type of `self`, as RFC 5545 3.8.2.2 requires of `DTEND`: a
    /// date end is exclusive, so an inclusive last day becomes the day after it; an instant
    /// end of a date start is the day after its date in `zone`.
    fn end_like(self, end: Self, inclusive: bool, zone: Tz) -> Self {
        match (self, end) {
            (Self::Date(_), Self::Date(e)) if inclusive => Self::Date(next_day(e)),
            (Self::Date(_), Self::Date(e)) => Self::Date(e),
            (Self::Date(_), other) => Self::Date(next_day(
                other
                    .utc_key(zone)
                    .and_utc()
                    .with_timezone(&zone)
                    .date_naive(),
            )),
            (_, Self::At(t)) => Self::At(t),
            (_, other) => Self::At(other.utc_past(zone).and_utc()),
        }
    }

    /// As `mode` asks, a floating time read in `zone`; `None` for a wall time the zone skips.
    fn read(self, mode: DateMode, zone: Tz) -> Option<Self> {
        let wall = match (mode, self) {
            (DateMode::Datetime, Self::Date(d)) => Self::Floating(d.and_time(NaiveTime::MIN)),
            (_, other) => other,
        };
        let zoned = match wall {
            Self::Floating(t) => Self::At(zone.from_local_datetime(&t).earliest()?.to_utc()),
            other => other,
        };
        Some(match (mode, zoned) {
            (DateMode::Date, Self::At(t)) => Self::Date(t.with_timezone(&zone).date_naive()),
            (_, other) => other,
        })
    }

    /// The property value, with `;VALUE=DATE` for a date: `DTSTART;VALUE=DATE:20261020`.
    fn property(self, name: &str) -> String {
        match self {
            Self::Date(d) => format!("{name};VALUE=DATE:{}", d.format("%Y%m%d")),
            // a read value is never floating (`Plan::when` zones it)
            other => format!(
                "{name}:{}",
                other.utc_key(chrono_tz::UTC).format("%Y%m%dT%H%M%SZ")
            ),
        }
    }
}

fn next_day(d: NaiveDate) -> NaiveDate {
    d.checked_add_days(Days::new(1)).unwrap_or(d)
}

/// The `from`/`to` of a feed, as UTC instants: an Event is in it when it starts before `to`
/// and has not ended before `from`; a bound left out does not narrow.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Window {
    /// The first instant served: a date's midnight.
    pub from: Option<NaiveDateTime>,
    /// Just past the last instant served: a date's next midnight, so `to` is inclusive.
    pub to: Option<NaiveDateTime>,
}

impl Window {
    /// The window of a feed's `from` and `to`, a date or a time without offset read in `zone`.
    #[must_use]
    pub fn read(from: Option<When>, to: Option<When>, zone: Tz) -> Self {
        Self {
            from: from.map(|w| w.utc_key(zone)),
            to: to.map(|w| w.utc_past(zone)),
        }
    }

    fn holds(&self, start: When, end: Option<When>, zone: Tz) -> bool {
        let begins = start.utc_key(zone);
        let ends = end
            .map_or_else(|| start.utc_past(zone), |e| e.utc_past(zone))
            .max(begins);
        self.to.is_none_or(|to| begins < to)
            && self.from.is_none_or(|from| ends > from || begins >= from)
    }
}

/// The `VEVENT` fields a mapping names, in the order a `VEVENT` writes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "SCREAMING-KEBAB-CASE")]
pub enum Field {
    /// §3.8.4.7, fixed: the entity id.
    Uid,
    /// §3.8.7.2, fixed: the entity's `modifiedAt`.
    Dtstamp,
    /// §3.8.2.4, required.
    Dtstart,
    /// §3.8.2.2.
    Dtend,
    /// §3.8.2.5.
    Duration,
    /// §3.8.1.12, required by the platform.
    Summary,
    /// §3.8.1.5.
    Description,
    /// §3.8.1.7.
    Location,
    /// §3.8.1.6.
    Geo,
    /// §3.8.4.6.
    Url,
    /// §3.8.1.2.
    Categories,
    /// §3.8.1.11.
    Status,
    /// §3.8.4.3.
    Organizer,
    /// §3.8.1.3, `PUBLIC` only.
    Class,
    /// §3.8.7.3; unmapped, the entity's `modifiedAt`.
    LastModified,
}

impl Field {
    /// Whether the field takes the remap `key`: the allow-list of API/02 §8b.
    fn allows(self, key: &str) -> bool {
        use Field as F;
        let text = matches!(
            self,
            F::Summary | F::Description | F::Location | F::Categories | F::Status
        );
        match key {
            "firstOf" => !matches!(self, F::Class),
            "join" => text && self != F::Status,
            "constant" => matches!(self, F::Class | F::Categories | F::Duration),
            "language" => text,
            "date" => matches!(self, F::Dtstart | F::Dtend | F::LastModified),
            "timezone" => matches!(self, F::Dtstart | F::Dtend),
            "endInclusive" => self == F::Dtend,
            "map" => matches!(self, F::Status | F::Categories),
            // `truncate`, the last key `compile_field` asks about
            _ => matches!(self, F::Summary | F::Description),
        }
    }
}

/// A named starting mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Preset {
    /// Smart Data Models `Event`, the default.
    SdmEvent,
    /// schema.org `Event`: `sdm-event` and `STATUS` from `eventStatus`.
    SchemaOrg,
}

/// How a date field reads its value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DateMode {
    /// As the value is.
    #[default]
    Auto,
    /// A whole day.
    Date,
    /// A date-time.
    Datetime,
}

/// The version of the `representations.ical` contract this broker reads (T-5207,
/// mapping-contract rule 1): a member added or given a new meaning takes the next one.
pub const MAPPING_VERSION: u64 = 1;

/// One named output of the view's `representations.ical` list, as the Portal writes it from
/// the Endpoint's `spec.representations.ical` (owner 2026-10-22T09:30Z and 09:40Z, the one
/// representation contract of T-5207): its sources, filter, titles and zone, served at
/// `{name}.ics`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
    /// The slug the output is served at.
    pub name: String,
    /// The contract version the Portal wrote; [`MAPPING_VERSION`] when absent.
    #[serde(default)]
    pub version: Option<u64>,
    /// The calendar's title per language tag: `NAME` (RFC 7986 5.1) and `X-WR-CALNAME`.
    #[serde(default)]
    pub title: BTreeMap<String, String>,
    /// The calendar's description per language tag: `DESCRIPTION` (RFC 7986 5.2).
    #[serde(default)]
    pub description: BTreeMap<String, String>,
    /// A query on top of the view that narrows the output, never widens it.
    #[serde(default)]
    pub filter: Filter,
    /// The language a `LanguageProperty` is read in when a field names none (BCP 47).
    #[serde(default)]
    pub lang: Option<String>,
    /// At most this many Events, 1 to 10000, the first in `sort` order.
    #[serde(default)]
    pub limit: Option<u32>,
    /// `start` (the default when `limit` is set) or `-start`: the Events by `DTSTART`.
    #[serde(default)]
    pub sort: Option<String>,
    /// The IANA zone a time without offset is read in and the calendar is shown in; default
    /// `Europe/Bratislava`.
    #[serde(default)]
    pub timezone: Option<String>,
    /// The model classes the output maps, each with its own preset or mapping; 1 to 10.
    pub sources: Vec<IcalMapping>,
}

/// One source of an [`Output`]: a model class and its field mapping.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IcalMapping {
    /// The contract version the Portal wrote; `1` when absent.
    #[serde(default = "first_version")]
    pub version: u64,
    /// The NGSI-LD type the feed maps, by its local name; default `Event`.
    #[serde(rename = "type", default = "event")]
    pub entity_type: String,
    /// The preset the mapping starts from; with no `mapping` either, `sdm-event`.
    #[serde(default)]
    pub preset: Option<Preset>,
    /// The fields that replace the preset's, or the whole mapping without a preset.
    #[serde(default)]
    pub mapping: BTreeMap<Field, Source>,
}

fn event() -> String {
    "Event".to_owned()
}

/// The types an iCal preset serves (owner 2026-10-22T09:25Z); another type maps `DTSTART`
/// and `SUMMARY` itself.
pub const PRESET_TYPES: [&str; 3] = ["Event", "CouncilMeeting", "OfficeHours"];

/// The output contract every format shares (docs API/11, T-5216).
pub use antares_ops::outputs::{is_language_tag, is_output_name, Filter, MAX_OUTPUTS};
use antares_ops::outputs::{is_name, is_plain};

/// The zone of an output that names none.
pub const DEFAULT_ZONE: Tz = chrono_tz::Europe::Bratislava;

/// The text of a `languageMap` for `want`: its exact tag, the same tag in another case, a tag
/// of the same primary language (`sk` for `sk-SK` and back), then `@none`, then the first tag
/// in code-point order.
fn pick<'a>(map: &'a serde_json::Map<String, Value>, want: Option<&str>) -> Option<&'a Value> {
    let primary = |t: &str| t.split('-').next().unwrap_or(t).to_ascii_lowercase();
    want.filter(|w| *w != "first")
        .and_then(|w| {
            map.get(w)
                .or_else(|| {
                    map.iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case(w))
                        .map(|(_, v)| v)
                })
                .or_else(|| {
                    map.iter()
                        .find(|(k, _)| primary(k) == primary(w))
                        .map(|(_, v)| v)
                })
        })
        .or_else(|| map.get("@none"))
        .or_else(|| map.values().next())
}

fn first_version() -> u64 {
    1
}

/// A written output of the view's `representations.ical` list, and whether it is degraded:
/// an output of a newer contract version than [`MAPPING_VERSION`] (the Portal released before
/// the broker) keeps its name, texts, language, zone, sort, limit, filter and each source's
/// type over its preset, and drops what this broker cannot read, instead of failing the
/// calendar (T-5207, mapping-contract rule 1). Its filter still has to read, since serving
/// the output without it would widen it. An output of this version is read strictly: the
/// Portal checked it when it was saved.
///
/// # Errors
/// Why an output of this version does not read, or why a newer one's filter does not.
///
/// ```
/// let newer = serde_json::json!({"name": "koncerty", "version": 9, "rrule": "x",
///     "sources": [{"type": "OfficeHours", "preset": "schema-org", "rrule": "eventSchedule"}]});
/// let (output, degraded) = antares_ical::mapping::output_of(&newer).unwrap();
/// assert!(degraded);
/// assert_eq!(output.sources[0].entity_type, "OfficeHours");
/// ```
pub fn output_of(written: &Value) -> Result<(Output, bool), String> {
    let version = written.get("version").and_then(Value::as_u64);
    if version.is_none_or(|v| v <= MAPPING_VERSION) {
        let output: Output = serde_json::from_value(written.clone()).map_err(|e| e.to_string())?;
        return Ok((output, false));
    }
    let text = |name: &str| written.get(name).and_then(Value::as_str).map(str::to_owned);
    let texts = |name: &str| {
        written
            .get(name)
            .and_then(|t| serde_json::from_value(t.clone()).ok())
            .unwrap_or_default()
    };
    let filter = match written.get("filter") {
        Some(f) => serde_json::from_value(f.clone()).map_err(|e| format!("filter: {e}"))?,
        None => Filter::default(),
    };
    let source = |s: &Value| IcalMapping {
        version: MAPPING_VERSION,
        entity_type: s
            .get("type")
            .and_then(Value::as_str)
            .filter(|t| is_name(t))
            .map_or_else(event, str::to_owned),
        preset: s
            .get("preset")
            .and_then(|p| serde_json::from_value(p.clone()).ok()),
        mapping: BTreeMap::new(),
    };
    let output = Output {
        name: text("name").unwrap_or_default(),
        version: None,
        title: texts("title"),
        description: texts("description"),
        filter,
        lang: text("lang"),
        limit: written
            .get("limit")
            .and_then(|l| serde_json::from_value(l.clone()).ok()),
        sort: text("sort").filter(|s| matches!(s.as_str(), "start" | "-start")),
        timezone: text("timezone").filter(|z| z.parse::<Tz>().is_ok()),
        sources: written
            .get("sources")
            .and_then(Value::as_array)
            .map(|list| list.iter().map(source).collect())
            .unwrap_or_default(),
    };
    tracing::warn!(version, name = %output.name, "newer calendar output: presets served");
    Ok((output, true))
}

/// A field's source: an attribute path (`startDate`) or the object form.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum Source {
    /// `DTSTART: startDate`.
    Attribute(String),
    /// `SUMMARY: {attribute: name, language: sk}`.
    Remap(Box<Remap>),
}

/// The object form of a [`Source`]: exactly one of `attribute`, `firstOf`, `join` and
/// `constant`, and the remaps its field allows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Remap {
    /// An attribute, or a dotted path into its value (`address.addressLocality`).
    #[serde(default)]
    pub attribute: Option<String>,
    /// The first of these attributes that has a value.
    #[serde(default)]
    pub first_of: Vec<String>,
    /// The values of these attributes that have one, joined by `separator`.
    #[serde(default)]
    pub join: Vec<String>,
    /// The `join` separator; default `", "`.
    #[serde(default)]
    pub separator: Option<String>,
    /// A fixed value.
    #[serde(default)]
    pub constant: Option<String>,
    /// Which text of a `LanguageProperty`: a BCP 47 tag or `first`; default the output's
    /// `lang`, then `@none`, then the first.
    #[serde(default)]
    pub language: Option<String>,
    /// How a date field reads its value.
    #[serde(default)]
    pub date: Option<DateMode>,
    /// The IANA zone a floating time is read in; the instant is written in UTC.
    #[serde(default)]
    pub timezone: Option<String>,
    /// The source names the last day, so a date `DTEND` is the day after.
    #[serde(default)]
    pub end_inclusive: Option<bool>,
    /// Source value to calendar value.
    #[serde(default)]
    pub map: BTreeMap<String, String>,
    /// The calendar value of a source value `map` does not name.
    #[serde(default)]
    pub default: Option<String>,
    /// At most this many characters.
    #[serde(default)]
    pub truncate: Option<u32>,
}

impl Source {
    fn remap(&self) -> Remap {
        match self {
            Self::Attribute(name) => Remap {
                attribute: Some(name.clone()),
                ..Remap::default()
            },
            Self::Remap(remap) => (**remap).clone(),
        }
    }
}

/// The `STATUS` values RFC 5545 §3.8.1.11 gives a `VEVENT`.
const STATUS_VALUES: [&str; 3] = ["TENTATIVE", "CONFIRMED", "CANCELLED"];

/// The skip reasons, as `antares_surface_records_skipped_total{reason}` names them.
pub mod reason {
    /// No value for `DTSTART`.
    pub const MISSING_DTSTART: &str = "missing-dtstart";
    /// No value for `SUMMARY`.
    pub const MISSING_SUMMARY: &str = "missing-summary";
    /// A date field holds a value that is no date.
    pub const UNPARSABLE_DATE: &str = "unparsable-date";
    /// `DURATION` holds a value that is no RFC 5545 duration.
    pub const UNPARSABLE_DURATION: &str = "unparsable-duration";
    /// `DTEND` is before an instant `DTSTART`, or not after a date one.
    pub const END_NOT_AFTER_START: &str = "dtend-not-after-dtstart";
    /// A `GEO` point outside -90..90 / -180..180.
    pub const GEO_OUT_OF_RANGE: &str = "geo-out-of-range";
    /// A `URL` that is not http(s) or holds white space.
    pub const URL_NOT_HTTP: &str = "url-not-http";
}

fn preset(preset: Preset) -> BTreeMap<Field, Remap> {
    let attribute = |name: &str| Remap {
        attribute: Some(name.to_owned()),
        ..Remap::default()
    };
    let mut fields: BTreeMap<_, _> = [
        (Field::Dtstart, "startDate"),
        (Field::Summary, "name"),
        (Field::Description, "description"),
        (Field::Location, "address"),
        (Field::Geo, "location"),
        (Field::Url, "url"),
        (Field::Organizer, "organizer"),
    ]
    .into_iter()
    .map(|(field, name)| (field, attribute(name)))
    .collect();
    // SDM and schema.org name the last day of a date range
    fields.insert(
        Field::Dtend,
        Remap {
            end_inclusive: Some(true),
            ..attribute("endDate")
        },
    );
    if preset == Preset::SchemaOrg {
        let map = [
            ("EventScheduled", "CONFIRMED"),
            ("EventRescheduled", "CONFIRMED"),
            ("EventMovedOnline", "CONFIRMED"),
            ("EventPostponed", "TENTATIVE"),
            ("EventCancelled", "CANCELLED"),
        ]
        .into_iter()
        .flat_map(|(from, to)| {
            [
                (from.to_owned(), to.to_owned()),
                (format!("https://schema.org/{from}"), to.to_owned()),
            ]
        })
        .collect();
        fields.insert(
            Field::Status,
            Remap {
                map,
                default: Some("CONFIRMED".to_owned()),
                ..attribute("eventStatus")
            },
        );
    }
    fields
}

fn is_path(path: &str) -> bool {
    path.split('.').count() <= 3 && path.split('.').all(is_name)
}

/// One field compiled: its remap and its zone.
#[derive(Debug, Clone)]
struct Compiled {
    remap: Remap,
    zone: Option<Tz>,
}

/// A mapping checked and resolved over its preset: what [`Plan::vevent`] renders.
#[derive(Debug, Clone)]
pub struct Plan {
    entity_type: String,
    fields: BTreeMap<Field, Compiled>,
    zone: Tz,
    lang: Option<String>,
}

impl Default for Plan {
    /// The `sdm-event` preset over `Event` in `Europe/Bratislava`.
    fn default() -> Self {
        Self {
            entity_type: event(),
            fields: preset(Preset::SdmEvent)
                .into_iter()
                .map(|(field, remap)| (field, Compiled { remap, zone: None }))
                .collect(),
            zone: DEFAULT_ZONE,
            lang: None,
        }
    }
}

impl Plan {
    /// The mapping in force (the preset's fields with the written ones over them), checked as
    /// API/02 §8b shapes it; the model check is the platform's, at propose.
    ///
    /// # Errors
    /// The first reason the mapping cannot be rendered.
    pub fn compile(mapping: &IcalMapping) -> Result<Self, String> {
        if mapping.version != MAPPING_VERSION {
            return Err(format!(
                "version {} is not one this broker reads ({MAPPING_VERSION})",
                mapping.version
            ));
        }
        if !is_name(&mapping.entity_type) {
            return Err(format!(
                "type {:?} is not an NGSI-LD type name",
                mapping.entity_type
            ));
        }
        let base = match (mapping.preset, mapping.mapping.is_empty()) {
            (Some(p), _) => preset(p),
            (None, true) => preset(Preset::SdmEvent),
            (None, false) => BTreeMap::new(),
        };
        let mut fields = BTreeMap::new();
        for (field, remap) in base {
            fields.insert(field, Compiled { remap, zone: None });
        }
        for (field, source) in &mapping.mapping {
            fields.insert(*field, compile_field(*field, source.remap())?);
        }
        for (field, name) in [(Field::Dtstart, "DTSTART"), (Field::Summary, "SUMMARY")] {
            if !fields.contains_key(&field) {
                return Err(format!("the mapping maps no {name}"));
            }
        }
        let fallback = fields
            .get(&Field::Duration)
            .is_some_and(|c| c.remap.constant.is_some());
        if fields.contains_key(&Field::Dtend) && fields.contains_key(&Field::Duration) && !fallback
        {
            return Err("the mapping maps both DTEND and DURATION".to_owned());
        }
        let own = |f: Field| mapping.mapping.contains_key(&f);
        let roles = own(Field::Dtstart) && own(Field::Summary);
        if !roles && !PRESET_TYPES.contains(&mapping.entity_type.as_str()) {
            return Err(format!(
                "type {} has no calendar preset: map its DTSTART and SUMMARY",
                mapping.entity_type
            ));
        }
        Ok(Self {
            entity_type: mapping.entity_type.clone(),
            fields,
            zone: DEFAULT_ZONE,
            lang: None,
        })
    }

    /// The zone a time without offset is read in and the calendar is shown in.
    #[must_use]
    pub fn zone(&self) -> Tz {
        self.zone
    }

    /// Where the Event starts as a UTC instant, for `sort`; `None` when it has no start.
    #[must_use]
    pub fn start(&self, entity: &Value) -> Option<NaiveDateTime> {
        self.when(entity, Field::Dtstart)
            .ok()
            .flatten()
            .map(|w| w.utc_key(self.zone))
    }
}

/// An [`Output`] checked: its plan per source, in the output's zone and language.
#[derive(Debug, Clone)]
pub struct CompiledOutput {
    /// The output as written.
    pub output: Output,
    /// The zone its times are read and shown in.
    pub zone: Tz,
    /// One plan per source, by type local name.
    pub plans: Vec<Plan>,
    /// Newest `DTSTART` first.
    pub descending: bool,
}

impl CompiledOutput {
    /// The output checked as API/02 §8b shapes it.
    ///
    /// # Errors
    /// The first reason the output cannot be served.
    pub fn compile(output: &Output) -> Result<Self, String> {
        let o = output;
        if o.version.is_some_and(|v| v != MAPPING_VERSION) {
            return Err(format!(
                "{}: version {:?} is not {MAPPING_VERSION}",
                o.name, o.version
            ));
        }
        if !is_output_name(&o.name) {
            return Err(format!(
                "name {:?} is not 1 to 40 of a-z, 0-9 and -, nor `preview`",
                o.name
            ));
        }
        let fail = |why: String| Err(format!("{}: {why}", o.name));
        let texts = o.title.iter().chain(&o.description);
        if let Some((tag, _)) = texts.clone().find(|(tag, _)| !is_language_tag(tag)) {
            return fail(format!("{tag:?} is not a language tag"));
        }
        if !texts.into_iter().all(|(_, text)| is_plain(text, 1000)) {
            return fail(
                "a title or description is empty, too long or holds control characters".to_owned(),
            );
        }
        if let Some(lang) = o.lang.as_ref().filter(|l| !is_language_tag(l)) {
            return fail(format!("lang {lang:?} is not a language tag"));
        }
        if o.limit.is_some_and(|n| n == 0 || n > 10_000) {
            return fail("limit is 1 to 10000".to_owned());
        }
        let descending = match o.sort.as_deref() {
            None | Some("start") => false,
            Some("-start") => true,
            Some(other) => return fail(format!("sort {other:?} is start or -start")),
        };
        if o.filter.params().iter().any(|(_, v)| !is_plain(v, 2000)) {
            return fail(
                "a filter member is empty, too long or holds control characters".to_owned(),
            );
        }
        if o.sources.is_empty() || o.sources.len() > 10 {
            return fail("sources lists 1 to 10 types".to_owned());
        }
        let zone = match &o.timezone {
            None => DEFAULT_ZONE,
            Some(name) => match name.parse::<Tz>() {
                Ok(z) => z,
                Err(_) => return fail(format!("timezone {name:?} is not an IANA zone")),
            },
        };
        let mut plans: Vec<Plan> = Vec::new();
        for source in &o.sources {
            let mut plan = Plan::compile(source).map_err(|why| format!("{}: {why}", o.name))?;
            if plans.iter().any(|p| p.entity_type == plan.entity_type) {
                return fail(format!("type {} is a source twice", plan.entity_type));
            }
            plan.zone = zone;
            plan.lang.clone_from(&o.lang);
            plans.push(plan);
        }
        Ok(Self {
            output: o.clone(),
            zone,
            plans,
            descending,
        })
    }

    /// The plan of an Entity's type, by its local name.
    #[must_use]
    pub fn plan(&self, entity: &Value) -> Option<&Plan> {
        let ty = local(
            entity
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        self.plans.iter().find(|p| p.entity_type == ty)
    }
}

impl Plan {
    /// The type the feed maps, by its local name.
    #[must_use]
    pub fn entity_type(&self) -> &str {
        &self.entity_type
    }
}

fn compile_field(field: Field, remap: Remap) -> Result<Compiled, String> {
    let fail = |why: &str| Err(format!("{field:?}: {why}"));
    if matches!(field, Field::Uid | Field::Dtstamp) {
        return fail("fixed by the broker, never mapped");
    }
    let sources = [
        remap.attribute.is_some(),
        !remap.first_of.is_empty(),
        !remap.join.is_empty(),
        remap.constant.is_some(),
    ];
    if sources.iter().filter(|on| **on).count() != 1 {
        return fail("needs exactly one of attribute, firstOf, join or constant");
    }
    let used = [
        ("firstOf", !remap.first_of.is_empty()),
        ("join", !remap.join.is_empty() || remap.separator.is_some()),
        ("constant", remap.constant.is_some()),
        ("language", remap.language.is_some()),
        ("date", remap.date.is_some()),
        ("timezone", remap.timezone.is_some()),
        ("endInclusive", remap.end_inclusive.is_some()),
        ("map", !remap.map.is_empty() || remap.default.is_some()),
        ("truncate", remap.truncate.is_some()),
    ];
    if let Some((key, _)) = used.iter().find(|(key, on)| *on && !field.allows(key)) {
        return fail(&format!("does not take `{key}`"));
    }
    let paths = remap
        .attribute
        .iter()
        .chain(&remap.first_of)
        .chain(&remap.join);
    if let Some(bad) = paths.clone().find(|p| !is_path(p)) {
        return fail(&format!("{bad:?} is not an attribute path"));
    }
    let plain = remap.separator.iter().all(|s| is_plain(s, 10))
        && remap.constant.iter().all(|c| is_plain(c, 100))
        && remap
            .map
            .iter()
            .all(|(from, to)| is_plain(from, 200) && is_plain(to, 100))
        && remap.default.iter().all(|d| is_plain(d, 100));
    if !plain {
        return fail("a separator, constant or map value is too long or holds control characters");
    }
    if field == Field::Class && remap.constant.as_deref() != Some("PUBLIC") {
        return fail("takes only `constant: PUBLIC`");
    }
    let status = remap.map.values().chain(&remap.default);
    if field == Field::Status && status.clone().any(|v| !STATUS_VALUES.contains(&v.as_str())) {
        return fail("takes TENTATIVE, CONFIRMED or CANCELLED");
    }
    if let Some(language) = &remap.language {
        if language != "first" && !is_language_tag(language) {
            return fail("`language` is a BCP 47 tag or `first`");
        }
    }
    if field == Field::Duration && remap.constant.as_deref().is_some_and(|d| !is_duration(d)) {
        return fail("`constant` is an RFC 5545 duration such as PT1H");
    }
    if remap.truncate.is_some_and(|n| n == 0 || n > 10_000) {
        return fail("`truncate` is 1 to 10000");
    }
    let zone = match &remap.timezone {
        None => None,
        Some(name) => Some(
            name.parse::<Tz>()
                .map_err(|_| format!("{field:?}: {name:?} is not an IANA zone"))?,
        ),
    };
    Ok(Compiled { remap, zone })
}

/// An attribute's instance: the default instance of a multi-instance one, else the first.
fn instance<'a>(entity: &'a Value, name: &str) -> Option<&'a Value> {
    let (_, member) = entity
        .as_object()?
        .iter()
        .find(|(key, _)| local(key) == name)?;
    match member {
        Value::Array(all) => all
            .iter()
            .find(|i| i.get("datasetId").is_none())
            .or_else(|| all.first()),
        one => Some(one),
    }
}

/// The members an attribute of each NGSI-LD 1.9 kind holds its value in: Property and
/// `GeoProperty`, `VocabProperty`, `Relationship`, `ListProperty`, `ListRelationship`, `JsonProperty`
/// (a `LanguageProperty`'s `languageMap` is read by [`pick`]).
const VALUE_MEMBERS: [&str; 6] = [
    "value",
    "vocab",
    "object",
    "valueList",
    "objectList",
    "json",
];

/// A Property's `value` (a typed literal's `@value`), a `VocabProperty`'s `vocab`, a
/// Relationship's `object`, a list's items, a `JsonProperty`'s `json`, a `LanguageProperty`'s
/// text in `language` ([`pick`]), a `GeoProperty`'s geometry.
fn value_of<'a>(instance: &'a Value, language: Option<&str>) -> Option<&'a Value> {
    if let Some(map) = instance.get("languageMap").and_then(Value::as_object) {
        return pick(map, language);
    }
    let value = VALUE_MEMBERS
        .iter()
        .find_map(|m| instance.get(*m))
        .unwrap_or(instance);
    Some(value.get("@value").unwrap_or(value))
}

/// The value at `path`: an attribute, then members of its value, or its sub-attributes.
fn at<'a>(entity: &'a Value, path: &str, language: Option<&str>) -> Option<&'a Value> {
    let mut names = path.split('.');
    let mut holder = instance(entity, names.next().unwrap_or(path))?;
    let mut value = value_of(holder, language)?;
    for name in names {
        if let Some(inner) = value.get(name) {
            value = inner;
        } else {
            holder = holder.get(name)?;
            value = value_of(holder, language)?;
        }
    }
    Some(value)
}

/// A value as non-empty text: a string trimmed, a number or a boolean.
fn text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.trim().to_owned()).filter(|t| !t.is_empty()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// A CAL-ADDRESS: a `mailto:` or http(s) URI as given, an e-mail address as `mailto:`, else none.
fn cal_address(value: String) -> Option<String> {
    if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    if ["mailto:", "https://", "http://"]
        .iter()
        .any(|s| value.starts_with(s))
    {
        Some(value)
    } else if value.contains('@') && !value.contains(':') {
        Some(format!("mailto:{value}"))
    } else {
        None
    }
}

/// RFC 5545 3.2 param-value, always quoted so `,` `;` `:` stay literal; a `"`, a `^` and a
/// newline are RFC 6868 caret escapes, the other control characters are dropped.
fn param(text: &str) -> String {
    let mut out = String::with_capacity(text.len().saturating_add(2));
    out.push('"');
    for c in text.chars() {
        match c {
            '^' => out.push_str("^^"),
            '"' => out.push_str("^'"),
            '\n' => out.push_str("^n"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A structured postal address as one line: street, postal code, locality and country.
fn address(value: &Value) -> Option<String> {
    let parts: Vec<String> = [
        "streetAddress",
        "postalCode",
        "addressLocality",
        "addressCountry",
    ]
    .iter()
    .filter_map(|k| value.get(k).and_then(text))
    .collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// RFC 5545 3.3.11 TEXT: backslash, semicolon, comma and newline escaped; the other control
/// characters, which TEXT excludes, dropped.
#[must_use]
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// RFC 5545 3.1: a content line of at most 75 octets, continued after CRLF and one space, cut
/// between characters and never inside one.
pub fn fold(line: &str, out: &mut String) {
    let mut width = 0usize;
    for c in line.chars() {
        let n = c.len_utf8();
        if width.saturating_add(n) > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width = width.saturating_add(n);
    }
    out.push_str("\r\n");
}

/// RFC 5545 3.3.6 `dur-value`: `P`, then weeks, or days and a time of hours, minutes and
/// seconds, an optional sign before.
fn is_duration(text: &str) -> bool {
    let body = text.strip_prefix(['+', '-']).unwrap_or(text);
    let Some(body) = body.strip_prefix('P') else {
        return false;
    };
    // each unit at most once, in the order `allowed` lists them, after at least one digit
    let units = |part: &str, allowed: &str| -> bool {
        let (mut rest, mut digits, mut any) = (allowed, 0usize, false);
        for c in part.chars() {
            if c.is_ascii_digit() {
                digits = digits.saturating_add(1);
                continue;
            }
            match rest.find(c) {
                Some(i) if digits > 0 => rest = rest.get(i.saturating_add(1)..).unwrap_or_default(),
                _ => return false,
            }
            (digits, any) = (0, true);
        }
        digits == 0 && any
    };
    match body.split_once('T') {
        Some((days, time)) => (days.is_empty() || units(days, "D")) && units(time, "HMS"),
        None => units(body, "W") || units(body, "D"),
    }
}

/// A source value through a field's `map`: its entry, else `default`, else itself when the
/// field maps nothing.
fn mapped(remap: &Remap, value: String) -> Option<String> {
    if remap.map.is_empty() && remap.default.is_none() {
        return Some(value);
    }
    remap.map.get(&value).or(remap.default.as_ref()).cloned()
}

/// What one Entity is in a feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Its `VEVENT`, folded, each line ending in CRLF.
    Event(String),
    /// A valid Event outside the window.
    Outside,
    /// No valid `VEVENT`: the [`reason`].
    Skipped(&'static str),
}

/// A field's value with the reason to skip when the entity holds one that does not read.
type Read<T> = Result<Option<T>, &'static str>;

impl Plan {
    fn field(&self, field: Field) -> Option<&Compiled> {
        self.fields.get(&field)
    }

    /// The field's text: its constant, the first source path holding text, or the joined ones.
    fn text(&self, entity: &Value, field: Field) -> Option<String> {
        let c = self.field(field)?;
        let remap = &c.remap;
        let language = remap.language.as_deref().or(self.lang.as_deref());
        let one = |path: &String| {
            at(entity, path, language).and_then(|v| match field {
                Field::Location if v.is_object() => address(v),
                _ => text(v),
            })
        };
        let value = if let Some(constant) = &remap.constant {
            Some(constant.clone())
        } else if remap.join.is_empty() {
            remap.attribute.iter().chain(&remap.first_of).find_map(one)
        } else {
            let parts: Vec<String> = remap.join.iter().filter_map(one).collect();
            let separator = remap.separator.as_deref().unwrap_or(", ");
            (!parts.is_empty()).then(|| parts.join(separator))
        }?;
        Some(match remap.truncate {
            Some(n) => value
                .chars()
                .take(usize::try_from(n).unwrap_or(usize::MAX))
                .collect(),
            None => value,
        })
    }

    /// A date field read: absent, a date, or unreadable.
    fn when(&self, entity: &Value, field: Field) -> Read<When> {
        let Some(c) = self.field(field) else {
            return Ok(None);
        };
        let Some(raw) = self.text(entity, field) else {
            return Ok(None);
        };
        When::parse(&raw)
            .and_then(|w| {
                w.read(
                    c.remap.date.unwrap_or_default(),
                    c.zone.unwrap_or(self.zone),
                )
            })
            .map(Some)
            .ok_or(reason::UNPARSABLE_DATE)
    }

    fn end(&self, entity: &Value, start: When) -> Read<When> {
        let Some(end) = self.when(entity, Field::Dtend)? else {
            return Ok(None);
        };
        let inclusive = self
            .field(Field::Dtend)
            .and_then(|c| c.remap.end_inclusive)
            .unwrap_or(false);
        let end = start.end_like(end, inclusive, self.zone);
        match end.utc_key(self.zone).cmp(&start.utc_key(self.zone)) {
            std::cmp::Ordering::Greater => Ok(Some(end)),
            // An instant event without `DTEND` ends at its start (RFC 5545 3.6.1): a source that
            // ends it there gets no `DTEND`, since 3.8.2.2 wants one later than `DTSTART`.
            std::cmp::Ordering::Equal if !matches!(start, When::Date(_)) => Ok(None),
            _ => Err(reason::END_NOT_AFTER_START),
        }
    }

    /// The `GEO` of a Point: latitude first (RFC 5545 3.8.1.6); another geometry has none.
    fn geo(&self, entity: &Value) -> Read<String> {
        let Some(c) = self.field(Field::Geo) else {
            return Ok(None);
        };
        let paths = c.remap.attribute.iter().chain(&c.remap.first_of);
        let Some(geometry) = paths
            .filter_map(|p| at(entity, p, None))
            .find(|g| g.is_object())
        else {
            return Ok(None);
        };
        if geometry.get("type").and_then(Value::as_str) != Some("Point") {
            return Ok(None);
        }
        let coordinates = geometry.get("coordinates").and_then(Value::as_array);
        let number = |i: usize| coordinates.and_then(|c| c.get(i)).and_then(Value::as_f64);
        let (Some(lon), Some(lat)) = (number(0), number(1)) else {
            return Ok(None);
        };
        if (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon) {
            Ok(Some(format!("GEO:{lat};{lon}")))
        } else {
            Err(reason::GEO_OUT_OF_RANGE)
        }
    }

    /// An http(s) `URL` without white space or control characters.
    fn url(&self, entity: &Value) -> Read<String> {
        let Some(url) = self.text(entity, Field::Url) else {
            return Ok(None);
        };
        let web = url.starts_with("https://") || url.starts_with("http://");
        if web && !url.chars().any(|c| c.is_whitespace() || c.is_control()) {
            Ok(Some(format!("URL:{url}")))
        } else {
            Err(reason::URL_NOT_HTTP)
        }
    }

    /// `CATEGORIES`: every text of the source (a list or one value), each through `map`.
    fn categories(&self, entity: &Value) -> Option<String> {
        let c = self.field(Field::Categories)?;
        let list = c
            .remap
            .attribute
            .as_ref()
            .and_then(|p| {
                at(
                    entity,
                    p,
                    c.remap.language.as_deref().or(self.lang.as_deref()),
                )
            })
            .and_then(Value::as_array);
        let values: Vec<String> = match list {
            // a ListRelationship's items are `{"object": id}`
            Some(items) => items
                .iter()
                .filter_map(|i| text(i.get("object").unwrap_or(i)))
                .collect(),
            None => self.text(entity, Field::Categories).into_iter().collect(),
        };
        let out: Vec<String> = values
            .into_iter()
            .filter_map(|v| mapped(&c.remap, v))
            .map(|v| escape(&v))
            .collect();
        (!out.is_empty()).then(|| format!("CATEGORIES:{}", out.join(",")))
    }

    fn status(&self, entity: &Value) -> Option<String> {
        let c = self.field(Field::Status)?;
        let value = self.text(entity, Field::Status)?;
        let value = mapped(&c.remap, value)?.to_ascii_uppercase();
        STATUS_VALUES
            .contains(&value.as_str())
            .then(|| format!("STATUS:{value}"))
    }

    /// `ORGANIZER`, a CAL-ADDRESS: the first source holding a `mailto:` or http(s) URI or an
    /// e-mail address, or an organizer object (schema.org `Organization`) whose `email`, else
    /// `url`, is the address and whose `name` becomes `CN`. An object without a usable address
    /// writes nothing: RFC 5545 requires the address, and the event's own `URL` is no organizer's.
    fn organizer(&self, entity: &Value) -> Option<String> {
        let c = self.field(Field::Organizer)?;
        let language = c.remap.language.as_deref().or(self.lang.as_deref());
        c.remap
            .attribute
            .iter()
            .chain(&c.remap.first_of)
            .filter_map(|path| at(entity, path, language))
            .find_map(|value| match value {
                Value::Object(o) => {
                    let member =
                        |k: &str| o.get(k).and_then(|v| value_of(v, language)).and_then(text);
                    let address = member("email")
                        .and_then(cal_address)
                        .or_else(|| member("url").and_then(cal_address))?;
                    Some(match member("name") {
                        Some(name) => format!("ORGANIZER;CN={}:{address}", param(&name)),
                        None => format!("ORGANIZER:{address}"),
                    })
                }
                _ => text(value)
                    .and_then(cal_address)
                    .map(|a| format!("ORGANIZER:{a}")),
            })
    }

    /// The latest `modifiedAt` of the attributes the mapping reads, every instance counted.
    fn revised(&self, entity: &Value) -> Option<DateTime<Utc>> {
        let names: std::collections::BTreeSet<&str> = self
            .fields
            .values()
            .flat_map(|c| {
                c.remap
                    .attribute
                    .iter()
                    .chain(&c.remap.first_of)
                    .chain(&c.remap.join)
            })
            .map(|p| p.split('.').next().unwrap_or(p))
            .collect();
        let stamp = |i: &Value| {
            i.get("modifiedAt")
                .and_then(Value::as_str)
                .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                .map(|t| t.to_utc())
        };
        entity
            .as_object()?
            .iter()
            .filter(|(key, _)| names.contains(local(key)))
            .flat_map(|(_, member)| match member {
                Value::Array(all) => all.iter().filter_map(stamp).collect::<Vec<_>>(),
                one => stamp(one).into_iter().collect(),
            })
            .max()
    }

    /// The `VEVENT` of one Entity, or why there is none; `now` stamps an Entity without
    /// `modifiedAt`.
    #[must_use]
    pub fn vevent(&self, entity: &Value, window: &Window, now: DateTime<Utc>) -> Outcome {
        match self.lines(entity, window, now) {
            Ok(Some(lines)) => {
                let mut out = String::new();
                for line in &lines {
                    fold(line, &mut out);
                }
                Outcome::Event(out)
            }
            Ok(None) => Outcome::Outside,
            Err(reason) => Outcome::Skipped(reason),
        }
    }

    fn lines(&self, entity: &Value, window: &Window, now: DateTime<Utc>) -> Read<Vec<String>> {
        // Every Entity a view answers has an id.
        let uid = entity.get("id").and_then(Value::as_str).unwrap_or_default();
        let start = self
            .when(entity, Field::Dtstart)?
            .ok_or(reason::MISSING_DTSTART)?;
        let summary = self
            .text(entity, Field::Summary)
            .ok_or(reason::MISSING_SUMMARY)?;
        let end = self.end(entity, start)?;
        // a constant DURATION is the length of an Event without an end; a date has its day
        let constant = self
            .field(Field::Duration)
            .is_some_and(|c| c.remap.constant.is_some());
        let duration = self
            .text(entity, Field::Duration)
            .filter(|_| !constant || (end.is_none() && !matches!(start, When::Date(_))));
        if duration.as_deref().is_some_and(|d| !is_duration(d)) {
            return Err(reason::UNPARSABLE_DURATION);
        }
        let (geo, url) = (self.geo(entity)?, self.url(entity)?);
        let entity_stamp = || {
            entity
                .get("modifiedAt")
                .and_then(Value::as_str)
                .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                .map(|t| t.to_utc())
        };
        let source = self
            .when(entity, Field::LastModified)?
            .map(|w| w.utc_key(self.zone).and_utc());
        let revised = source
            .or_else(|| self.revised(entity))
            .or_else(entity_stamp);
        let modified = match source {
            Some(t) => Some(When::At(t)),
            None if self.field(Field::LastModified).is_some() => None,
            None => revised.map(When::At),
        };
        if !window.holds(start, end, self.zone) {
            return Ok(None);
        }
        let stamp = revised.unwrap_or(now);
        let text = |field: Field, name: &str| {
            self.text(entity, field)
                .map(|t| format!("{name}:{}", escape(&t)))
        };
        let mut lines = vec![
            "BEGIN:VEVENT".to_owned(),
            format!("UID:{}", escape(uid)),
            When::At(stamp).property("DTSTAMP"),
            start.property("DTSTART"),
        ];
        lines.extend(end.map(|e| e.property("DTEND")));
        lines.extend(duration.map(|d| format!("DURATION:{d}")));
        lines.push(format!("SUMMARY:{}", escape(&summary)));
        lines.extend(text(Field::Description, "DESCRIPTION"));
        lines.extend(text(Field::Location, "LOCATION"));
        lines.extend(geo);
        lines.extend(url);
        lines.extend(self.categories(entity));
        lines.extend(self.status(entity));
        lines.extend(self.organizer(entity));
        lines.extend(text(Field::Class, "CLASS"));
        lines.extend(modified.map(|m| m.property("LAST-MODIFIED")));
        lines.push("END:VEVENT".to_owned());
        Ok(Some(lines))
    }
}
