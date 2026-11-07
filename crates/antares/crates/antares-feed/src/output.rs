// SPDX-License-Identifier: EUPL-1.2
//! A view's feed outputs (`representations.feed`, platform API/02 §8b, EP-150, EP-151) and one
//! Entity read through an output's role mapping as one feed entry.
//!
//! An output is data, never code: each role names attribute paths, the first with a value
//! winning. An Entity without a title or an `updated` time it can read is no entry, and its
//! reason is counted (EP-126). Text is kept as text; the writers escape it.

use antares_ops::outputs::is_name;
use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// The most outputs one view lists (EP-151).
pub const MAX_OUTPUTS: usize = 20;

/// The largest page an output or a request may ask for (EP-150).
pub const MAX_LIMIT: usize = 200;

/// The page size of an output that names none.
pub const DEFAULT_LIMIT: usize = 50;

/// The contract version this broker reads (T-5207).
pub const VERSION: u32 = 1;

/// The skip reasons, as `antares_surface_records_skipped_total{reason}` names them.
pub const NO_TITLE: &str = "no-title";
/// See [`NO_TITLE`].
pub const NO_UPDATED: &str = "no-updated";

/// One output of `representations.feed`, as the gateway writes it from the Endpoint's
/// `spec.representations.feed`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
    /// A slug, unique on the view; the URL segment after `feed/v1/`.
    pub name: String,
    /// The contract version, [`VERSION`] when left out.
    #[serde(default)]
    pub version: Option<u32>,
    /// The feed's title per language.
    #[serde(default)]
    pub title: BTreeMap<String, String>,
    /// The feed's description per language.
    #[serde(default)]
    pub description: BTreeMap<String, String>,
    /// The model classes the feed lists, each with its roles.
    pub sources: Vec<Source>,
    /// What narrows the view for this output.
    #[serde(default)]
    pub filter: Filter,
    /// The feed's language when the request names none.
    #[serde(default)]
    pub lang: Option<String>,
    /// The default page size, 1 to [`MAX_LIMIT`].
    #[serde(default)]
    pub limit: Option<usize>,
    /// The order: `updated`, newest first, the only one.
    #[serde(default)]
    pub sort: Option<Sort>,
}

/// The order of a feed's entries.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Sort {
    /// Newest first by the `updated` role.
    Updated,
}

/// One model class of an output.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    /// The NGSI-LD type, by its short name or IRI.
    #[serde(rename = "type")]
    pub ty: String,
    /// The starting roles.
    #[serde(default)]
    pub preset: Option<Preset>,
    /// Roles on top of the preset, role by role.
    #[serde(default)]
    pub mapping: BTreeMap<Role, Paths>,
}

/// A named starting mapping.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    /// Smart Data Models `Event`.
    Event,
    /// An article or a notice (schema.org `Article`).
    Article,
    /// Any type with a `name`.
    Generic,
}

/// The roles of an entry.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// The entry's title (required).
    Title,
    /// A short text.
    Summary,
    /// The whole text.
    Content,
    /// The page the entry is about.
    Link,
    /// When the entry last changed (required).
    Updated,
    /// When the entry first appeared.
    Published,
    /// Who wrote or organizes it.
    Author,
    /// Its categories, one per value.
    Category,
    /// An image of it.
    Image,
}

/// One attribute path, or a list of them: the first with a value wins.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Paths {
    /// One path.
    One(String),
    /// The first of several with a value.
    Many(Vec<String>),
}

impl Paths {
    fn list(self) -> Vec<String> {
        match self {
            Self::One(p) => vec![p],
            Self::Many(ps) => ps,
        }
    }
}

pub use antares_ops::outputs::Filter;

/// Whether the filter fixes the area a request may then not name: a `georel`, `geometry` or
/// `coordinates` of its own. A `geoproperty` alone names which `GeoProperty` a request's geo
/// query reads.
#[must_use]
pub fn fixes_area(filter: &Filter) -> bool {
    [&filter.georel, &filter.geometry, &filter.coordinates]
        .iter()
        .any(|o| o.is_some())
}

/// An output checked: its sources' roles resolved over their presets.
#[derive(Debug, Clone)]
pub struct Plan {
    /// The output's name.
    pub name: String,
    /// Its titles and descriptions per language.
    pub title: BTreeMap<String, String>,
    /// See [`Plan::title`].
    pub description: BTreeMap<String, String>,
    /// Each source type's local name and its roles.
    pub sources: Vec<(String, BTreeMap<Role, Vec<String>>)>,
    /// Its filter.
    pub filter: Filter,
    /// Its default language.
    pub lang: Option<String>,
    /// Its default page size.
    pub limit: usize,
}

/// The last segment of an expanded IRI, or the term itself.
#[must_use]
pub fn local(name: &str) -> &str {
    name.rsplit(['/', '#']).next().unwrap_or(name)
}

/// A slug: `[a-z0-9]` then `[a-z0-9-]`, 1 to 63.
fn is_slug(name: &str) -> bool {
    (1..=63).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-')
}

/// A type: a name, or an http(s) IRI.
fn is_type(ty: &str) -> bool {
    is_name(ty)
        || ((ty.starts_with("https://") || ty.starts_with("http://"))
            && ty.len() <= 512
            && !ty.chars().any(|c| c.is_whitespace() || c.is_control()))
}

/// A dotted attribute path: names joined by `.`.
fn is_path(path: &str) -> bool {
    path.split('.').all(is_name)
}

fn preset(preset: Preset) -> BTreeMap<Role, Vec<String>> {
    let roles: &[(Role, &[&str])] = match preset {
        Preset::Event => &[
            (Role::Title, &["name"]),
            (Role::Updated, &["modifiedAt"]),
            (Role::Published, &["createdAt"]),
            (Role::Summary, &["description"]),
            (Role::Link, &["url"]),
            (Role::Author, &["organizer"]),
            (Role::Category, &["category"]),
            (Role::Image, &["image"]),
        ],
        Preset::Article => &[
            (Role::Title, &["headline", "name"]),
            (Role::Updated, &["dateModified", "modifiedAt"]),
            (Role::Published, &["datePublished", "dateCreated"]),
            (Role::Summary, &["description"]),
            (Role::Content, &["articleBody"]),
            (Role::Link, &["url"]),
            (Role::Author, &["author"]),
            (Role::Category, &["category"]),
            (Role::Image, &["image"]),
        ],
        Preset::Generic => &[
            (Role::Title, &["name"]),
            (Role::Updated, &["modifiedAt"]),
            (Role::Summary, &["description"]),
            (Role::Link, &["url"]),
        ],
    };
    roles
        .iter()
        .map(|(role, paths)| (*role, paths.iter().map(|p| (*p).to_owned()).collect()))
        .collect()
}

impl Plan {
    /// A written output compiled, and whether it was degraded: an output of a newer contract
    /// version than [`VERSION`] keeps its name, texts, language, limit, filter and each
    /// source's type and preset, and drops what this broker cannot read (T-5207 rule 1).
    /// Its filter still has to read, since serving one without it would widen the output.
    ///
    /// # Errors
    /// As [`Plan::compile`], and a newer output whose filter or presets do not read.
    pub fn of(written: &Value) -> Result<(Self, bool), String> {
        let version = written.get("version").and_then(Value::as_u64);
        if version.is_none_or(|v| v <= u64::from(VERSION)) {
            let output: Output =
                serde_json::from_value(written.clone()).map_err(|e| e.to_string())?;
            return Self::compile(output).map(|p| (p, false));
        }
        let field = |name: &str| written.get(name).cloned().unwrap_or(Value::Null);
        let texts = |name: &str| serde_json::from_value(field(name)).unwrap_or_default();
        let filter = match written.get("filter") {
            Some(f) => serde_json::from_value(f.clone()).map_err(|e| format!("filter: {e}"))?,
            None => Filter::default(),
        };
        let sources = written
            .get("sources")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .map(|s| Source {
                        ty: s
                            .get("type")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        preset: s
                            .get("preset")
                            .and_then(|p| serde_json::from_value(p.clone()).ok()),
                        mapping: BTreeMap::new(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let output = Output {
            name: written
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            version: None,
            title: texts("title"),
            description: texts("description"),
            sources,
            filter,
            lang: written
                .get("lang")
                .and_then(Value::as_str)
                .map(str::to_owned),
            limit: serde_json::from_value(field("limit")).unwrap_or_default(),
            sort: None,
        };
        tracing::warn!(version, name = %output.name, "newer feed output: presets served");
        Self::compile(output).map(|p| (p, true))
    }

    /// The output checked, or the sentence naming what does not hold.
    ///
    /// # Errors
    /// A name that is no slug, another version, no source or more than 20, a type or a path
    /// that is no NGSI-LD name, a source without `title` or `updated`, or a `limit` outside 1
    /// to [`MAX_LIMIT`].
    pub fn compile(output: Output) -> Result<Self, String> {
        if !is_slug(&output.name) {
            return Err("name is a slug of a-z, 0-9 and -, 1 to 63 characters".into());
        }
        if output.version.is_some_and(|v| v != VERSION) {
            return Err(format!("version {VERSION} is the one this broker reads"));
        }
        if output.sources.is_empty() || output.sources.len() > MAX_OUTPUTS {
            return Err(format!("sources lists 1 to {MAX_OUTPUTS} types"));
        }
        let limit = output.limit.unwrap_or(DEFAULT_LIMIT);
        if !(1..=MAX_LIMIT).contains(&limit) {
            return Err(format!("limit is 1 to {MAX_LIMIT}"));
        }
        let mut sources = Vec::with_capacity(output.sources.len());
        for source in output.sources {
            if !is_type(&source.ty) {
                return Err(format!("{:?} is not an NGSI-LD type", source.ty));
            }
            let mut roles = source.preset.map(preset).unwrap_or_default();
            for (role, paths) in source.mapping {
                let paths = paths.list();
                if paths.is_empty() || !paths.iter().all(|p| is_path(p)) {
                    return Err(format!(
                        "{role:?} names attribute paths such as name or address.addressLocality"
                    ));
                }
                roles.insert(role, paths);
            }
            for required in [Role::Title, Role::Updated] {
                if !roles.contains_key(&required) {
                    return Err(format!(
                        "{} has no {required:?} role: name a preset or map it",
                        source.ty
                    ));
                }
            }
            sources.push((local(&source.ty).to_owned(), roles));
        }
        Ok(Self {
            name: output.name,
            title: output.title,
            description: output.description,
            sources,
            filter: output.filter,
            lang: output.lang,
            limit,
        })
    }

    /// The local names of the types the output lists.
    pub fn types(&self) -> impl Iterator<Item = &str> {
        self.sources.iter().map(|(ty, _)| ty.as_str())
    }

    /// The roles of an Entity's type, if the output lists it.
    fn roles(&self, entity: &Value) -> Option<&BTreeMap<Role, Vec<String>>> {
        let ty = local(entity.get("type").and_then(Value::as_str)?);
        self.sources.iter().find(|(t, _)| t == ty).map(|(_, r)| r)
    }

    /// The Entity as an entry, or the reason it is none; `None` for a type the output does
    /// not list.
    #[must_use]
    pub fn entry(&self, entity: &Value, lang: Option<&str>) -> Option<Result<Entry, &'static str>> {
        let roles = self.roles(entity)?;
        let id = entity.get("id").and_then(Value::as_str)?;
        let texts = |role: Role| -> Vec<String> {
            roles
                .get(&role)
                .and_then(|paths| {
                    paths
                        .iter()
                        .map(|p| texts(at(entity, p, lang)))
                        .find(|t| !t.is_empty())
                })
                .unwrap_or_default()
        };
        let first = |role: Role| texts(role).into_iter().next();
        let time = |role: Role| first(role).as_deref().and_then(instant);
        let Some(title) = first(Role::Title) else {
            return Some(Err(NO_TITLE));
        };
        let Some(updated) = time(Role::Updated) else {
            return Some(Err(NO_UPDATED));
        };
        Some(Ok(Entry {
            entity: id.to_owned(),
            title,
            summary: first(Role::Summary),
            content: first(Role::Content),
            link: first(Role::Link).filter(|l| is_http(l)),
            updated,
            published: time(Role::Published),
            authors: texts(Role::Author),
            categories: texts(Role::Category),
            image: first(Role::Image).filter(|l| is_http(l)),
        }))
    }
}

/// One feed entry, text unescaped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The Entity id.
    pub entity: String,
    /// Its title.
    pub title: String,
    /// Its summary.
    pub summary: Option<String>,
    /// Its content.
    pub content: Option<String>,
    /// Its http(s) link.
    pub link: Option<String>,
    /// When it last changed.
    pub updated: DateTime<Utc>,
    /// When it appeared.
    pub published: Option<DateTime<Utc>>,
    /// Its authors.
    pub authors: Vec<String>,
    /// Its categories.
    pub categories: Vec<String>,
    /// Its http(s) image.
    pub image: Option<String>,
}

/// An absolute http(s) URL without whitespace or control characters (safeHref).
#[must_use]
pub fn is_http(url: &str) -> bool {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    rest.is_some_and(|r| {
        !r.is_empty()
            && !r.starts_with('/')
            && !url.chars().any(|c| c.is_whitespace() || c.is_control())
    })
}

/// An RFC 3339 date-time, or a date read as its midnight in UTC.
#[must_use]
pub fn instant(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .map(|t| t.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDate::parse_from_str(text, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|t| t.and_utc())
        })
}

/// An attribute's instance: the default instance of a multi-instance one, else the first.
fn instance<'a>(entity: &'a Value, name: &str) -> Option<&'a Value> {
    match entity.get(name)? {
        Value::Array(all) => all
            .iter()
            .find(|i| i.get("datasetId").is_none())
            .or_else(|| all.first()),
        one => Some(one),
    }
}

/// A Property's `value` (a typed literal's `@value`), a `LanguageProperty`'s text in `lang`
/// (else its first), a Relationship's `object`; a system member (`modifiedAt`) as it is.
fn value_of<'a>(instance: &'a Value, lang: Option<&str>) -> Option<&'a Value> {
    let Value::Object(member) = instance else {
        return Some(instance);
    };
    if let Some(Value::Object(map)) = member.get("languageMap") {
        return lang
            .and_then(|l| map.get(l))
            .or_else(|| map.values().next());
    }
    let value = member.get("value").or_else(|| member.get("object"))?;
    Some(value.get("@value").unwrap_or(value))
}

/// The value at `path`: an attribute, then members of its value, or its sub-attributes.
fn at<'a>(entity: &'a Value, path: &str, lang: Option<&str>) -> Option<&'a Value> {
    let mut names = path.split('.');
    let attribute = instance(entity, names.next()?)?;
    let mut value = value_of(attribute, lang)?;
    let mut holder = attribute;
    for name in names {
        let next = value.get(name).or_else(|| holder.get(name))?;
        holder = next;
        value = value_of(next, lang)?;
    }
    Some(value)
}

/// A text trimmed, without the control characters a feed cannot carry (XML 1.0 2.2).
fn clean(text: &str) -> String {
    text.trim()
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\t' | '\n' | '\r'))
        .collect()
}

/// A value as texts: a string trimmed, a number or a boolean, each of an array's.
fn texts(value: Option<&Value>) -> Vec<String> {
    let one = |v: &Value| match v {
        Value::String(s) => Some(clean(s)).filter(|s| !s.is_empty()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    };
    match value {
        Some(Value::Array(all)) => all.iter().filter_map(one).collect(),
        Some(v) => one(v).into_iter().collect(),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn plan(output: &Value) -> Result<Plan, String> {
        Plan::of(output).map(|(p, _)| p)
    }

    #[test]
    fn a_newer_output_is_served_from_its_presets_and_keeps_its_filter() -> Result<(), String> {
        let (p, degraded) = Plan::of(&json!({"name": "sport", "version": 2, "limit": 20,
            "title": {"sk": "Šport"}, "layout": "cards", "filter": {"q": "category==\"sport\""},
            "sources": [{"type": "Event", "preset": "event", "mapping": {"title": {"next": 1}}}]}))?;
        assert!(degraded);
        assert_eq!(p.name, "sport");
        assert_eq!(p.limit, 20);
        assert_eq!(p.title.get("sk").map(String::as_str), Some("Šport"));
        assert_eq!(p.filter.q.as_deref(), Some("category==\"sport\""));
        assert_eq!(p.types().collect::<Vec<_>>(), ["Event"]);
        let (_, current) = Plan::of(&json!({"name": "x", "version": 1,
            "sources": [{"type": "Event", "preset": "event"}]}))?;
        assert!(!current);
        for bad in [
            json!({"name": "x", "version": 2, "filter": {"where": "x"},
                "sources": [{"type": "Event", "preset": "event"}]}),
            json!({"name": "x", "version": 2, "sources": [{"type": "Event", "preset": "next"}]}),
            json!({"name": "x", "version": 2}),
            json!({"version": 2, "sources": [{"type": "Event", "preset": "event"}]}),
        ] {
            assert!(Plan::of(&bad).is_err(), "{bad}");
        }
        Ok(())
    }

    #[test]
    fn an_output_without_title_or_updated_or_with_a_bad_name_does_not_compile() {
        let ok = json!({"name": "vsetky", "sources": [{"type": "Event", "preset": "event"}]});
        assert!(plan(&ok).is_ok());
        for bad in [
            json!({"name": "Vsetky", "sources": [{"type": "Event", "preset": "event"}]}),
            json!({"name": "-x", "sources": [{"type": "Event", "preset": "event"}]}),
            json!({"name": "x", "sources": []}),
            json!({"name": "x", "sources": [{"type": "Event"}]}),
            json!({"name": "x", "sources": [{"type": "Event", "mapping": {"title": "name"}}]}),
            json!({"name": "x", "sources": [{"type": "Event", "preset": "event", "mapping": {"title": "a b"}}]}),
            json!({"name": "x", "sources": [{"type": "a b", "preset": "event"}]}),
            json!({"name": "x", "limit": 201, "sources": [{"type": "Event", "preset": "event"}]}),
            json!({"name": "x", "version": 0, "sources": [{"type": "Event", "preset": "event"}]}),
            json!({"name": "x", "sort": "published", "sources": [{"type": "Event", "preset": "event"}]}),
            json!({"name": "x", "unknown": 1, "sources": [{"type": "Event", "preset": "event"}]}),
            json!({"name": "x", "filter": {"where": "x"}, "sources": [{"type": "Event", "preset": "event"}]}),
        ] {
            assert!(plan(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn roles_read_paths_languages_and_the_first_with_a_value() -> Result<(), String> {
        let p = plan(
            &json!({"name": "a", "sources": [{"type": "Article", "preset": "article",
            "mapping": {"author": "author.name"}}]}),
        )?;
        let entity = json!({"id": "urn:ngsi-ld:Article:1", "type": "Article",
            "modifiedAt": "2026-10-01T10:00:00Z",
            "name": {"type": "LanguageProperty", "languageMap": {"sk": "Správa", "en": "News"}},
            "datePublished": {"type": "Property", "value": {"@type": "Date", "@value": "2026-09-30"}},
            "author": {"type": "Property", "value": {"name": "Mesto"}},
            "category": {"type": "Property", "value": ["a", "b"]},
            "url": {"type": "Property", "value": "javascript:alert(1)"}});
        let e = p.entry(&entity, Some("en")).ok_or("not listed")??;
        assert_eq!(e.title, "News");
        assert_eq!(e.updated, instant("2026-10-01T10:00:00Z").ok_or("time")?);
        assert_eq!(e.published, instant("2026-09-30"));
        assert_eq!(e.authors, ["Mesto"]);
        assert_eq!(e.categories, ["a", "b"]);
        assert_eq!(e.link, None);
        let untitled =
            json!({"id": "urn:x:2", "type": "Article", "modifiedAt": "2026-10-01T10:00:00Z"});
        assert_eq!(p.entry(&untitled, None), Some(Err(NO_TITLE)));
        let undated =
            json!({"id": "urn:x:3", "type": "Article", "name": {"type": "Property", "value": "x"}});
        assert_eq!(p.entry(&undated, None), Some(Err(NO_UPDATED)));
        assert_eq!(
            p.entry(&json!({"id": "urn:x:4", "type": "Other"}), None),
            None
        );
        Ok(())
    }

    #[test]
    fn names_types_and_paths_are_checked_at_every_edge() {
        let event = |name: &str, ty: &str, title: Value| json!({"name": name, "sources": [{"type": ty, "preset": "generic", "mapping": {"title": title}}]});
        let long_path = "a".repeat(129);
        let long_iri = format!("https://a.sk/{}", "x".repeat(512));
        for ok in [
            event("sport2", "Event", json!("name")),
            event(
                "a",
                "https://smartdatamodels.org/dataModel.Event/Event",
                json!(["headline", "name"]),
            ),
            event("a", "http://schema.org/Event", json!("_x.y1")),
        ] {
            assert!(plan(&ok).is_ok(), "{ok}");
        }
        for bad in [
            event("", "Event", json!("name")),
            event("a", "Event", json!(long_path)),
            event("a", "Event", json!("1a")),
            event("a", "Event", json!([])),
            event("a", "1Event", json!("name")),
            event("a", &long_iri, json!("name")),
            event("a", "https://a.sk/a b", json!("name")),
            event("a", "https://a.sk/\u{7}", json!("name")),
        ] {
            assert!(plan(&bad).is_err(), "{bad}");
        }
        let many: Vec<Value> = (0..=MAX_OUTPUTS)
            .map(|_| json!({"type": "Event", "preset": "event"}))
            .collect();
        assert!(plan(&json!({"name": "a", "sources": many})).is_err());
    }

    #[test]
    fn instances_languages_and_scalar_values_read_as_text() -> Result<(), String> {
        let p = plan(&json!({"name": "a", "sources": [{"type": "Event", "preset": "event"}]}))?;
        let filter = json!({"georel": "near;maxDistance==1"});
        assert!(serde_json::from_value::<Filter>(filter).is_ok_and(|f| fixes_area(&f)));
        assert!(!fixes_area(&Filter::default()));
        let geoproperty = Filter {
            geoproperty: Some("location".into()),
            ..Filter::default()
        };
        assert!(!fixes_area(&geoproperty));
        let entity = json!({"id": "urn:ngsi-ld:Event:1", "type": "Event", "modifiedAt": "2026-10-01T10:00:00Z",
            "name": [{"type": "LanguageProperty", "datasetId": "urn:x", "languageMap": {"en": "Other"}},
                     {"type": "LanguageProperty", "languageMap": {"sk": "Koncert"}}],
            "description": [{"type": "Property", "datasetId": "urn:a", "value": "first"},
                            {"type": "Property", "datasetId": "urn:b", "value": "second"}],
            "category": {"type": "Property", "value": [1, true, {"x": 1}, " ", "a\u{1}b"]},
            "organizer": {"type": "Relationship", "object": "urn:ngsi-ld:Organization:1"}});
        let e = p.entry(&entity, None).ok_or("not listed")??;
        assert_eq!(e.title, "Koncert");
        assert_eq!(e.summary.as_deref(), Some("first"));
        assert_eq!(e.categories, ["1", "true", "ab"]);
        assert_eq!(e.authors, ["urn:ngsi-ld:Organization:1"]);
        Ok(())
    }

    #[test]
    fn only_absolute_http_links_pass() {
        for ok in ["https://a.sk/x", "http://a.sk"] {
            assert!(is_http(ok), "{ok}");
        }
        for bad in [
            "javascript:x",
            "https:///x",
            "https://",
            "ftp://a",
            "https://a b",
            "/rel",
        ] {
            assert!(!is_http(bad), "{bad}");
        }
    }
}
