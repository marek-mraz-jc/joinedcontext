// SPDX-License-Identifier: EUPL-1.2
//! The named outputs of a view's `representations.<format>` list (docs API/11 EP-159, T-5216):
//! one contract every format reads, the Portal writing it from the Endpoint's
//! `spec.representations.<format>`. An output maps one or more model classes by roles, may
//! narrow the view with a `filter`, and is served under its `name`. A filter only narrows: its
//! `q` and `scopeQ` are AND-ed with a caller's and a fixed geo-query is never replaced, while
//! the view and the Policy still decide per Entity. An output that does not hold serves
//! nobody, and the other outputs of the list still serve (EP-156).

use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// The most named outputs one representation lists (owner 2026-10-22T09:30Z).
pub const MAX_OUTPUTS: usize = 20;

/// The most model classes one output maps.
pub const MAX_SOURCES: usize = 10;

/// The roles a source of `ogc` and `geojson` maps: the geometry (a `GeoProperty`: `location`,
/// `observationSpace`, `operationSpace` or one of the model's), the time `instant` or the
/// interval `start` and `end`, and the served `properties` (all of them when not mapped).
pub const GEO_ROLES: [&str; 5] = ["geometry", "instant", "start", "end", "properties"];

/// The role a source of `csv` maps: its `columns` in order (all of them when not mapped).
pub const CSV_ROLES: [&str; 1] = ["columns"];

/// The role a source of `sta` maps: the attributes that become Datastreams (every numeric
/// Property when not mapped, T-5225).
pub const STA_ROLES: [&str; 1] = ["datastreams"];

/// The formats whose output reads one model class, its geo-query the caller's: `ogc` (a
/// collection) and `odata` (an `EntitySet` of one `EntityType`, T-5225), which maps no role.
const ONE_CLASS: [&str; 2] = ["ogc", "odata"];

/// The NGSI-LD query an output adds to every read through the view (TS 104 175 6.4.3.2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Filter {
    /// `q`, AND-ed with a caller's.
    #[serde(default)]
    pub q: Option<String>,
    /// `scopeQ`, AND-ed with a caller's.
    #[serde(default, rename = "scopeQ")]
    pub scope_q: Option<String>,
    /// `georel`; with it the output fixes the geo-query and a caller names none.
    #[serde(default)]
    pub georel: Option<String>,
    /// `geometry`.
    #[serde(default)]
    pub geometry: Option<String>,
    /// `coordinates`.
    #[serde(default)]
    pub coordinates: Option<String>,
    /// `geoproperty`.
    #[serde(default)]
    pub geoproperty: Option<String>,
}

/// The query parameters of a geo-query (TS 104 175 6.4.3.2).
const GEO: [&str; 4] = ["georel", "geometry", "coordinates", "geoproperty"];

impl Filter {
    /// The members set, by their query parameter names.
    #[must_use]
    pub fn params(&self) -> Vec<(&'static str, &str)> {
        [
            ("q", &self.q),
            ("scopeQ", &self.scope_q),
            ("georel", &self.georel),
            ("geometry", &self.geometry),
            ("coordinates", &self.coordinates),
            ("geoproperty", &self.geoproperty),
        ]
        .into_iter()
        .filter_map(|(k, v)| v.as_deref().map(|v| (k, v)))
        .collect()
    }

    /// Whether the filter names a geo-query.
    #[must_use]
    pub fn has_geo(&self) -> bool {
        self.params().iter().any(|(k, _)| GEO.contains(k))
    }

    /// The `scopeQ` that selects what both the filter's and the caller's `theirs` select
    /// (TS 104 175 4.19), either alone when the other names none.
    ///
    /// # Errors
    /// When the two cannot be joined: a malformed one, or a product too long to ask.
    pub fn scope_with(&self, theirs: Option<&str>) -> Result<Option<String>, &'static str> {
        match (self.scope_q.as_deref(), theirs) {
            (Some(ours), Some(theirs)) => antares_ql::scope::intersect_scope_q(ours, theirs)
                .map(Some)
                .ok_or("this output's scopeQ and the request's cannot be joined"),
            (ours, theirs) => Ok(ours.or(theirs).map(str::to_owned)),
        }
    }

    /// The caller's query parameters `params` under the filter: `q` and `scopeQ` AND-ed as
    /// `(filter);(caller's)`, a fixed geo-query never replaced.
    ///
    /// # Errors
    /// When the filter fixes a geo-query and the caller names a member of one, or the filter
    /// has a `q` and the caller's is no expression of its own ([`one_q`]).
    pub fn narrow(&self, params: &mut BTreeMap<String, String>) -> Result<(), &'static str> {
        if self.georel.is_some() && GEO.iter().any(|k| params.contains_key(*k)) {
            return Err(
                "this output fixes its geo-query: georel, geometry, coordinates and geoproperty are its own",
            );
        }
        if let (Some(ours), Some(theirs)) = (&self.q, params.get("q")) {
            // F-ANT4-007: the output's own q must be one group as well, or
            // `a==1)|(b==2` joined as `(a==1)|(b==2);(theirs)` ORs past it
            one_q(ours).map_err(|_| "this output's q is no query expression of its own")?;
            one_q(theirs)?;
        }
        if let Some(scope) = self.scope_with(params.get("scopeQ").map(String::as_str))? {
            params.insert("scopeQ".to_owned(), scope);
        }
        for (key, value) in self.params() {
            let merged = match (key, params.get(key)) {
                ("q", Some(theirs)) => format!("({value});({theirs})"),
                ("scopeQ", _) => continue,
                _ => value.to_owned(),
            };
            params.insert(key.to_owned(), merged);
        }
        Ok(())
    }
}

/// Refuses a caller's `q` that does not parse alone (TS 104 175 4.9), so the text joined as
/// `(filter);(q)` is one group: `b==2)|(c==3` would close the filter's group and OR past it.
///
/// # Errors
/// When `q` is no query of its own.
pub fn one_q(q: &str) -> Result<(), &'static str> {
    antares_ql::parse_q(q).map(drop).map_err(|_| {
        "q is no query expression of its own (TS 104 175 4.9): check its parentheses and quotes"
    })
}

/// What a role reads: an attribute path, or a list of paths (of which the first with a value
/// wins, or for `properties` and `columns` the ones served, in order).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum Paths {
    /// `geometry: location`.
    One(String),
    /// `geometry: [operationSpace, location]`.
    Many(Vec<String>),
}

impl Paths {
    /// The paths in order.
    #[must_use]
    pub fn all(&self) -> Vec<&str> {
        match self {
            Self::One(p) => vec![p.as_str()],
            Self::Many(ps) => ps.iter().map(String::as_str).collect(),
        }
    }
}

/// One source of an [`Output`]: a model class, by its local name, and the roles it maps.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleSource {
    /// The NGSI-LD type, by its local name.
    #[serde(rename = "type")]
    pub entity_type: String,
    /// Role to the attribute paths it reads; a role not mapped takes the format's default.
    #[serde(default)]
    pub mapping: BTreeMap<String, Paths>,
}

impl RoleSource {
    /// The paths of `role`, none when it is not mapped.
    #[must_use]
    pub fn role(&self, role: &str) -> Vec<&str> {
        self.mapping.get(role).map(Paths::all).unwrap_or_default()
    }
}

/// One named output of `representations.<format>` for the formats that map by roles (ogc,
/// geojson, csv); iCal and the feeds read their own fields over the same members.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
    /// The slug the output is served at.
    pub name: String,
    /// The contract version; a newer one is served from the members this broker reads (EP-153).
    #[serde(default)]
    pub version: Option<u64>,
    /// The output's title per language tag.
    #[serde(default)]
    pub title: BTreeMap<String, String>,
    /// The output's description per language tag.
    #[serde(default)]
    pub description: BTreeMap<String, String>,
    /// A query on top of the view that narrows the output, never widens it.
    #[serde(default)]
    pub filter: Filter,
    /// The language a `LanguageProperty` is read in when the caller names none (BCP 47).
    #[serde(default)]
    pub lang: Option<String>,
    /// The page size (`ogc`, `geojson`) or the most rows (`csv`): 1 to 10000.
    #[serde(default)]
    pub limit: Option<u32>,
    /// The model classes the output maps; 1 to [`MAX_SOURCES`].
    pub sources: Vec<RoleSource>,
}

impl Output {
    /// The output's title in `lang` (its exact tag, then `en`, then the first), else its name.
    #[must_use]
    pub fn title_in(&self, lang: &str) -> &str {
        self.title
            .get(lang)
            .or_else(|| self.title.get("en"))
            .or_else(|| self.title.values().next())
            .unwrap_or(&self.name)
    }

    /// The output's description in `lang`, as [`Output::title_in`] picks it.
    #[must_use]
    pub fn description_in(&self, lang: &str) -> Option<&str> {
        self.description
            .get(lang)
            .or_else(|| self.description.get("en"))
            .or_else(|| self.description.values().next())
            .map(String::as_str)
    }

    /// The output checked for `format` (`ogc`, `geojson`, `csv`, `sta` or `odata`).
    ///
    /// # Errors
    /// The first member that does not hold, named.
    pub fn check(self, format: &str) -> Result<Self, String> {
        let roles: &[&str] = match format {
            "csv" => &CSV_ROLES,
            "sta" => &STA_ROLES,
            "odata" => &[],
            _ => &GEO_ROLES,
        };
        let one = ONE_CLASS.contains(&format);
        let texts = self.title.iter().chain(&self.description);
        let checks = [
            (is_output_name(&self.name), "name is 1 to 40 of a-z, 0-9 and -, not preview".to_owned()),
            (self.version != Some(0), "version is 1 or more".to_owned()),
            (
                texts.clone().all(|(tag, text)| is_language_tag(tag) && is_plain(text, 1000)),
                "a title or description is keyed by no language tag, empty, too long or holds control characters".to_owned(),
            ),
            (self.lang.as_deref().is_none_or(is_language_tag), "lang is a language tag".to_owned()),
            (self.limit.is_none_or(|n| (1..=10_000).contains(&n)), "limit is 1 to 10000".to_owned()),
            (
                self.filter.params().iter().all(|(_, v)| is_plain(v, 2000)),
                "a filter member is empty, too long or holds control characters".to_owned(),
            ),
            (
                !one || !self.filter.has_geo(),
                format!("an {format} output narrows by q and scopeQ; the geo-query is the caller's"),
            ),
            (
                (1..=if one { 1 } else { MAX_SOURCES }).contains(&self.sources.len()),
                format!("sources lists 1 to {} types for {format}", if one { 1 } else { MAX_SOURCES }),
            ),
        ];
        if let Some((_, why)) = checks.into_iter().find(|(ok, _)| !ok) {
            return Err(format!("{}: {why}", self.name));
        }
        for (i, source) in self.sources.iter().enumerate() {
            let twice = self
                .sources
                .iter()
                .take(i)
                .any(|s| s.entity_type == source.entity_type);
            let bad_role = source.mapping.keys().find(|r| !roles.contains(&r.as_str()));
            let bad_path = source
                .mapping
                .values()
                .flat_map(Paths::all)
                .find(|p| !is_path(p));
            let why = if !is_path(&source.entity_type) || source.entity_type.contains('.') {
                format!("type {:?} is no type name", source.entity_type)
            } else if twice {
                format!("type {} is a source twice", source.entity_type)
            } else if let Some(role) = bad_role {
                format!(
                    "{} maps {role:?}, which is none of {roles:?}",
                    source.entity_type
                )
            } else if let Some(path) = bad_path {
                format!(
                    "{} maps {path:?}, which is no attribute path",
                    source.entity_type
                )
            } else {
                continue;
            };
            return Err(format!("{}: {why}", self.name));
        }
        Ok(self)
    }
}

/// The outputs `listed` under one format (a view's `representations.<format>`), each by its
/// name and checked for `format`; an element without a string `name` is skipped. A list of
/// more than [`MAX_OUTPUTS`] or with a name twice, which the gateway refuses at save, serves
/// nobody.
#[must_use]
pub fn listed(listed: Option<&Value>, format: &str) -> Vec<(String, Result<Output, String>)> {
    judged(listed, format, None)
}

/// The output `name` of `listed`, judged as [`listed`] judges it, without
/// reading the other elements of the list: a request names one output, and
/// reading all [`MAX_OUTPUTS`] of them was the cost of every such request
/// (R3-025).
#[must_use]
pub fn one(listed: Option<&Value>, format: &str, name: &str) -> Option<Result<Output, String>> {
    judged(listed, format, Some(name)).pop().map(|(_, o)| o)
}

fn judged(
    listed: Option<&Value>,
    format: &str,
    only: Option<&str>,
) -> Vec<(String, Result<Output, String>)> {
    let all: Vec<&Value> = listed
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    // judged over the elements served: one without a name is skipped, not a
    // reason to refuse the named ones
    let mut names: Vec<&str> = all
        .iter()
        .filter_map(|o| o.get("name").and_then(Value::as_str))
        .collect();
    let listed_names = names.len();
    names.sort_unstable();
    names.dedup();
    let whole = listed_names <= MAX_OUTPUTS && names.len() == listed_names;
    all.into_iter()
        .filter_map(|o| {
            let name = o.get("name")?.as_str()?;
            if only.is_some_and(|n| n != name) {
                return None;
            }
            let name = name.to_owned();
            let output = serde_json::from_value::<Output>(o.clone())
                .map_err(|e| format!("{name}: {e}"))
                .and_then(|o| o.check(format))
                .and_then(|o| {
                    whole.then_some(o).ok_or(format!(
                        "{name}: the list holds more than {MAX_OUTPUTS} outputs or a name twice"
                    ))
                });
            Some((name, output))
        })
        .collect()
}

/// An output name: `[a-z0-9-]`, 1 to 40, not `preview`.
#[must_use]
pub fn is_output_name(name: &str) -> bool {
    (1..=40).contains(&name.len())
        && name != "preview"
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A BCP 47 language tag by its shape (`sk`, `sk-SK`, `zh-Hant`), or `@none`.
#[must_use]
pub fn is_language_tag(tag: &str) -> bool {
    let mut parts = tag.split('-');
    tag == "@none"
        || (tag.len() <= 35
            && parts.next().is_some_and(|p| {
                (2..=3).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphabetic())
            })
            && parts.all(|p| {
                (1..=8).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphanumeric())
            }))
}

/// An attribute or a dotted path into its value: 1 to 200 of letters, digits, `_`, `-`, `.`
/// and `:`.
fn is_path(path: &str) -> bool {
    (1..=200).contains(&path.len())
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-.:".contains(c))
}

/// Whether an id is an absolute URI (RFC 3986 section 3.1: a scheme, then `:`), as every
/// NGSI-LD Entity id is.
#[must_use]
pub fn is_uri(id: &str) -> bool {
    id.split_once(':').is_some_and(|(scheme, _)| {
        scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}

/// An NGSI-LD name: a letter or `_`, then letters, digits and `_`, at most 128.
#[must_use]
pub fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 128
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Text a person reads: not empty, at most `max` characters, no control character.
#[must_use]
pub fn is_plain(value: &str, max: usize) -> bool {
    !value.is_empty() && value.chars().count() <= max && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_name_is_a_letter_or_underscore_then_word_characters() {
        for good in ["a", "_x", "speed_2", &"n".repeat(128)] {
            assert!(is_name(good), "{good}");
        }
        for bad in ["", "2a", "a-b", "a.b", "é", &"n".repeat(129)] {
            assert!(!is_name(bad), "{bad}");
        }
    }

    /// R3-025: `one` judges the named output as `listed` does, the rest of
    /// the list read only for its names.
    #[test]
    fn one_output_is_judged_as_the_list_judges_it() {
        let car = json!({"name": "a", "sources": [{"type": "Car"}]});
        let list = json!([car, {"name": "b", "sources": 7}, {"sources": []}]);
        let whole = listed(Some(&list), "sta");
        for name in ["a", "b"] {
            let want = whole
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, o)| o.is_ok());
            assert_eq!(
                one(Some(&list), "sta", name).map(|o| o.is_ok()),
                want,
                "{name}"
            );
        }
        assert!(one(Some(&list), "sta", "c").is_none());
        let twice = json!([car, car]);
        assert!(one(Some(&twice), "sta", "a").is_some_and(|o| o.is_err()));
    }

    fn checked(output: &Value, format: &str) -> Result<Output, String> {
        listed(Some(&json!([output])), format)
            .pop()
            .map_or(Err("skipped".to_owned()), |(_, o)| o)
    }

    /// F-ANT4-007: a filter `q` that closes its own group would OR past the
    /// caller's once joined as `(filter);(caller's)`: the join is refused.
    #[test]
    fn a_filter_q_that_is_no_group_of_its_own_is_never_joined() {
        let filter = Filter {
            q: Some("a==1)|(b==2".to_owned()),
            ..Filter::default()
        };
        let mut params = BTreeMap::from([("q".to_owned(), "c==3".to_owned())]);
        assert!(filter.narrow(&mut params).is_err(), "{params:?}");
        let fine = Filter {
            q: Some("a==1".to_owned()),
            ..Filter::default()
        };
        assert!(fine.narrow(&mut params).is_ok());
        assert_eq!(params.get("q").map(String::as_str), Some("(a==1);(c==3)"));
    }

    /// F-ANT4-009: an element without a name is skipped; it does not make
    /// the named one beside it a list "with a name twice".
    #[test]
    fn a_nameless_element_does_not_refuse_the_named_ones() {
        let list = json!([{"name": "a", "sources": [{"type": "Event"}]}, {"title": {"en": "x"}}]);
        let out = listed(Some(&list), "geojson");
        assert_eq!(out.len(), 1);
        assert_eq!(
            out.first().map(|(n, o)| (n.as_str(), o.is_ok())),
            Some(("a", true))
        );
    }

    #[test]
    fn an_output_reads_its_members_and_names_what_does_not_hold() {
        let good = json!({"name": "venues", "version": 2, "title": {"sk": "Miesta", "en": "Venues"},
            "description": {"sk": "Kde"}, "lang": "sk", "limit": 50, "filter": {"q": "a==1"},
            "sources": [{"type": "Event", "mapping": {"geometry": ["operationSpace", "location"],
            "instant": "startDate", "properties": ["name"]}}]});
        let o = checked(&good, "ogc").expect("holds");
        assert_eq!(o.title_in("sk"), "Miesta");
        assert_eq!(o.title_in("de"), "Venues");
        assert_eq!(o.description_in("de"), Some("Kde"));
        let first = o.sources.first().expect("one source");
        assert_eq!(first.role("geometry"), vec!["operationSpace", "location"]);
        assert_eq!(first.role("instant"), vec!["startDate"]);
        assert_eq!(first.role("end"), Vec::<&str>::new());
        let bare =
            checked(&json!({"name": "a", "sources": [{"type": "Event"}]}), "csv").expect("holds");
        assert_eq!(
            (bare.title_in("sk"), bare.description_in("sk")),
            ("a", None)
        );
        let one = |title: Value| json!({"name": "a", "title": title, "sources": [{"type": "E"}]});
        assert_eq!(
            checked(&one(json!({"de": "x"})), "csv").map(|o| o.title_in("sk").to_owned()),
            Ok("x".to_owned())
        );
    }

    #[test]
    fn an_output_names_the_member_that_does_not_hold() {
        let source = |s: Value| json!({"name": "a", "sources": [s]});
        let long = "x".repeat(1001);
        let refused = [
            (
                json!({"name": "A", "sources": [{"type": "E"}]}),
                "ogc",
                "name is",
            ),
            (
                json!({"name": "a", "version": 0, "sources": [{"type": "E"}]}),
                "ogc",
                "version",
            ),
            (
                json!({"name": "a", "title": {"x": "t"}, "sources": [{"type": "E"}]}),
                "ogc",
                "title or description",
            ),
            (
                json!({"name": "a", "description": {"sk": ""}, "sources": [{"type": "E"}]}),
                "ogc",
                "title or description",
            ),
            (
                json!({"name": "a", "title": {"sk": long}, "sources": [{"type": "E"}]}),
                "ogc",
                "title or description",
            ),
            (
                json!({"name": "a", "lang": "slovak", "sources": [{"type": "E"}]}),
                "ogc",
                "lang",
            ),
            (
                json!({"name": "a", "limit": 0, "sources": [{"type": "E"}]}),
                "ogc",
                "limit",
            ),
            (
                json!({"name": "a", "filter": {"q": ""}, "sources": [{"type": "E"}]}),
                "ogc",
                "filter member",
            ),
            (
                json!({"name": "a", "filter": {"georel": "near;maxDistance==1"}, "sources": [{"type": "E"}]}),
                "ogc",
                "geo-query is the caller's",
            ),
            (json!({"name": "a", "sources": []}), "geojson", "1 to 10"),
            (
                json!({"name": "a", "sources": [{"type": "E"}, {"type": "F"}]}),
                "ogc",
                "1 to 1",
            ),
            (source(json!({"type": "a.b"})), "ogc", "no type name"),
            (source(json!({"type": ""})), "ogc", "no type name"),
            (
                json!({"name": "a", "sources": [{"type": "E"}, {"type": "E"}]}),
                "csv",
                "twice",
            ),
            (
                source(json!({"type": "E", "mapping": {"columns": "x"}})),
                "ogc",
                "none of",
            ),
            (
                source(json!({"type": "E", "mapping": {"geometry": "x"}})),
                "csv",
                "none of",
            ),
            (
                source(json!({"type": "E", "mapping": {"geometry": "a b"}})),
                "geojson",
                "no attribute path",
            ),
            (
                source(json!({"type": "E", "preset": "x"})),
                "geojson",
                "unknown field",
            ),
            (
                json!({"name": "a", "sort": "start", "sources": [{"type": "E"}]}),
                "geojson",
                "unknown field",
            ),
        ];
        for (output, format, why) in refused {
            let hit = checked(&output, format).err().map(|e| e.contains(why));
            assert_eq!(hit, Some(true), "{output} {format} {why}");
        }
        for nothing in [
            json!([{"title": {}}, {"name": 1}, "x"]),
            json!({"name": "a"}),
        ] {
            assert_eq!(listed(Some(&nothing), "ogc").len(), 0, "{nothing}");
        }
        assert_eq!(listed(None, "ogc").len(), 0);
    }

    #[test]
    fn an_sta_output_maps_its_datastreams_and_an_odata_one_reads_one_class() {
        let source = |s: Value| json!({"name": "a", "sources": [s]});
        let near =
            json!({"georel": "near;maxDistance==1", "geometry": "Point", "coordinates": "[17,48]"});
        let refused = [
            (
                json!({"name": "a", "filter": near, "sources": [{"type": "E"}]}),
                "odata",
                "geo-query is the caller's",
            ),
            (
                json!({"name": "a", "sources": [{"type": "E"}, {"type": "F"}]}),
                "odata",
                "1 to 1",
            ),
            (
                source(json!({"type": "E", "mapping": {"properties": "x"}})),
                "odata",
                "none of",
            ),
            (
                source(json!({"type": "E", "mapping": {"geometry": "x"}})),
                "sta",
                "none of",
            ),
        ];
        for (output, format, why) in refused {
            let hit = checked(&output, format).err().map(|e| e.contains(why));
            assert_eq!(hit, Some(true), "{output} {format} {why}");
        }
        let sta = json!({"name": "air", "filter": near,
            "sources": [{"type": "A", "mapping": {"datastreams": ["pm10"]}}, {"type": "B"}]});
        let o = checked(&sta, "sta").expect("an sta output holds a geo-query and several sources");
        assert_eq!(
            o.sources.first().map(|s| s.role("datastreams")),
            Some(vec!["pm10"])
        );
        assert!(checked(&json!({"name": "a", "sources": [{"type": "E"}]}), "odata").is_ok());
    }

    #[test]
    fn a_list_the_gateway_refuses_at_save_serves_nobody() {
        let one = json!({"name": "a", "sources": [{"type": "E"}]});
        let many: Vec<Value> = (0..=MAX_OUTPUTS)
            .map(|i| json!({"name": format!("o{i}"), "sources": [{"type": "E"}]}))
            .collect();
        for list in [json!([one, one]), json!(many)] {
            let served = listed(Some(&list), "csv")
                .iter()
                .filter(|(_, o)| o.is_ok())
                .count();
            assert_eq!(served, 0, "{list}");
        }
    }

    #[test]
    fn a_filter_only_narrows_the_callers_query() {
        let filter = Filter {
            q: Some("a==1".into()),
            scope_q: Some("/city".into()),
            ..Filter::default()
        };
        let mut params = BTreeMap::from([
            ("q".to_owned(), "b==2".to_owned()),
            ("lang".to_owned(), "sk".to_owned()),
        ]);
        assert_eq!(filter.narrow(&mut params), Ok(()));
        assert_eq!(params.get("q").map(String::as_str), Some("(a==1);(b==2)"));
        assert_eq!(params.get("scopeQ").map(String::as_str), Some("/city"));
        let mut theirs = BTreeMap::from([("scopeQ".to_owned(), "/city/a,/town".to_owned())]);
        assert_eq!(filter.narrow(&mut theirs), Ok(()));
        let joined = theirs.get("scopeQ").map(String::as_str);
        assert_eq!(joined, Some("(/city;/city/a),(/city;/town)"));
        let mut malformed = BTreeMap::from([("scopeQ".to_owned(), ",".to_owned())]);
        assert!(filter.narrow(&mut malformed).is_err());
        assert!(!filter.has_geo());
        let geo = Filter {
            georel: Some("near;maxDistance==100".into()),
            geometry: Some("Point".into()),
            coordinates: Some("[17,48]".into()),
            geoproperty: Some("location".into()),
            ..Filter::default()
        };
        assert!(geo.has_geo());
        let mut theirs = BTreeMap::from([("geometry".to_owned(), "Polygon".to_owned())]);
        assert!(geo.narrow(&mut theirs).is_err());
        let mut none = BTreeMap::new();
        assert_eq!(geo.narrow(&mut none), Ok(()));
        assert_eq!(none.len(), 4);
        let unfixed = Filter {
            geoproperty: Some("place".into()),
            ..Filter::default()
        };
        let mut theirs = BTreeMap::from([("geoproperty".to_owned(), "location".to_owned())]);
        assert_eq!(unfixed.narrow(&mut theirs), Ok(()));
        assert_eq!(theirs.get("geoproperty").map(String::as_str), Some("place"));
    }

    #[test]
    fn a_callers_q_never_closes_the_filters_group() {
        let filter = Filter {
            q: Some("a==1".into()),
            ..Filter::default()
        };
        for escape in ["b==2)|(c==3", "b==2)|(c==3;(d==4", ")", "b==\"2"] {
            let mut params = BTreeMap::from([("q".to_owned(), escape.to_owned())]);
            assert!(filter.narrow(&mut params).is_err(), "{escape}");
            assert_eq!(params.get("q").map(String::as_str), Some(escape));
        }
        // nothing to join: the broker judges the caller's q alone, with its own answer
        let unfiltered = Filter::default();
        let mut params = BTreeMap::from([("q".to_owned(), "b==2)|(c==3".to_owned())]);
        assert_eq!(unfiltered.narrow(&mut params), Ok(()));
        let mut quoted = BTreeMap::from([("q".to_owned(), "name==\"a)|(b\"".to_owned())]);
        assert_eq!(filter.narrow(&mut quoted), Ok(()));
        let joined = quoted.get("q").map(String::as_str);
        assert_eq!(joined, Some("(a==1);(name==\"a)|(b\")"));
    }

    #[test]
    fn names_and_language_tags_by_their_shape() {
        for name in ["", "preview", "Sport", "a_b", &"a".repeat(41)] {
            assert!(!is_output_name(name), "{name:?}");
        }
        assert!(is_output_name("okres-zvolen-2"));
        for tag in ["sk", "sk-SK", "zh-Hant", "@none"] {
            assert!(is_language_tag(tag), "{tag}");
        }
        for tag in [
            "",
            "s",
            "slovak",
            "sk-",
            "sk-toolongpart",
            "1k",
            "sk-aaaaaaaa-bbbbbbbb-cccccccc-dddddddd",
        ] {
            assert!(!is_language_tag(tag), "{tag}");
        }
    }
}
