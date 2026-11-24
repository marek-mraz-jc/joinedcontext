//! `kind: Portal`: one open data portal of an Organization, declared in `portals/` of the
//! Organization repository (ODP-08..ODP-11).
//!
//! An Organization holds several portals; each one is a dataroom catalogue the reconciler
//! creates, updates and deletes. The portal whose `spec.path` is [`DEFAULT_PORTAL`] is the
//! Organization's own catalogue at `/{org}/opendata/`, kept with its catalogue id (ODP-09);
//! every other one answers at `/{org}/opendata/-/{path}/` or on a host the operator listed for
//! the Organization. `metadata.name` is the manifest's key in namespace `org`, unique across
//! the deployment; `spec.path` is the name inside the Organization, unique there. What needs
//! other manifests or the deployment configuration (a unique path, a listed host,
//! `portals.max`) is the Portal's judge; this kind checks the manifest alone.

use crate::envelope::{Kind, ObjectMeta, Scope};
use crate::error::{Error, Result};
use crate::kinds::OrganizationAppearance;
use crate::names;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The profile every catalogue serves, chosen or not.
pub const BASE_PROFILE: &str = "dcat-ap-3.0.1";

/// The `spec.path` of an Organization's own portal, at `/{org}/opendata/` (ODP-09).
pub const DEFAULT_PORTAL: &str = "default";

/// The most sources one portal harvests or lists.
pub const MAX_PORTAL_SOURCES: usize = 32;

/// How a portal on its own host answers its slug address `/{org}/opendata/-/{path}/` (ODP-08).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SlugAddress {
    /// The slug address answers `308` to the same path and query on the host.
    #[default]
    Redirect,
    /// Both addresses answer; every page and feed names the host as canonical.
    Keep,
}

/// Where a [`PortalSource`] takes its datasets from (ODP-10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PortalSourceType {
    /// This Organization's already published Endpoints, narrowed by project, space, tag or name.
    Endpoints,
    /// A DCAT-AP catalogue (Turtle, RDF/XML or JSON-LD), e.g. another deployment's `catalog.ttl`.
    DcatAp,
    /// A CKAN Action API (`.../api/3/action`).
    CkanAction,
    /// The Slovak national catalogue's SPARQL endpoint.
    NkodSparql,
    /// An ArcGIS Hub site.
    ArcgisHub,
    /// An ArcGIS REST services directory, folder, FeatureServer or MapServer, e.g.
    /// `https://gis.example.sk/arcgis/rest/services`: one record per service.
    ArcgisRest,
    /// An OGC API Records collection, e.g. `https://data.example.sk/collections/metadata`.
    OgcRecords,
}

/// The most a harvest run of a source reads before it fails and changes nothing (ODP-13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct HarvestBudget {
    /// Records per run, e.g. `5000`.
    pub records: u64,
    /// Bytes read per run, e.g. `50000000`.
    pub bytes: u64,
}

/// One source of a portal (ODP-10). No source carries a credential in this version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PortalSource {
    /// Unique in the portal, a DNS label, e.g. `kosice`.
    pub name: String,
    /// The source's type, e.g. `dcat-ap`.
    #[serde(rename = "type")]
    pub kind: PortalSourceType,
    /// `endpoints`: only these projects' Endpoints.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<String>,
    /// `endpoints`: only these spaces' Endpoints.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spaces: Vec<String>,
    /// `endpoints`: only Endpoints carrying one of these tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// `endpoints`: only these Endpoints by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
    /// A harvested catalogue's address, `https://` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Five-field cron, UTC, e.g. `23 3 * * *`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<String>,
    /// Every host a run may fetch from, the `url`'s own included.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_hosts: Vec<String>,
    /// Records and bytes one run may read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<HarvestBudget>,
    /// The CKAN organisation the records land in, e.g. `kosice`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    /// The publisher's title, e.g. `Mesto Košice`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher_title: Option<String>,
    /// The publisher's IRI, the one the records name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher_uri: Option<String>,
    /// The publisher's description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher_description: Option<String>,
    /// The IRI of the area the records cover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spatial_uri: Option<String>,
    /// A prefix for each record's name, e.g. `ke-`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_prefix: Option<String>,
    /// Hosts in harvested URLs replaced by others, e.g. `{old.sk: new.sk}`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rewrite_hosts: BTreeMap<String, String>,
    /// Items left out (title, IRI or item id) with the reason the run log gives.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub skip: BTreeMap<String, String>,
    /// Titles replaced, old title to new.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub titles: BTreeMap<String, String>,
    /// Spelling fixed in titles and descriptions, wrong to right.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub spelling: BTreeMap<String, String>,
    /// A column each named dataset must have, dataset to column.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub require_column: BTreeMap<String, String>,
    /// The public base the Endpoints of converted datasets answer under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway: Option<String>,
    /// The deployment's file of converted datasets beside jc.toml, e.g. `od-datasets.json`: only
    /// those datasets are listed, with their Endpoint's distributions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub converted_file: Option<String>,
    /// The catalogue base the converted file's dataset URLs name, replaced by this portal's own,
    /// e.g. `https://opendata.dev.joinedcontext.com/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datasets_base: Option<String>,
    /// Whether the source runs on its schedule; `false` keeps it and its records without runs.
    #[serde(default = "enabled", skip_serializing_if = "is_enabled")]
    pub enabled: bool,
}

const fn enabled() -> bool {
    true
}

// serde's skip_serializing_if passes a reference.
const fn is_enabled(on: &bool) -> bool {
    *on
}

/// Desired specification of a [`Portal`][crate::kinds::Portal] (ODP-08).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PortalSpec {
    /// The Organization the portal belongs to, e.g. `bratislava`.
    pub organization_ref: String,
    /// The portal's name inside the Organization, a DNS label such as `mobility`:
    /// `/{org}/opendata/-/mobility/`; `default` is the Organization's own portal.
    pub path: String,
    /// The title on every page, per language, e.g. `{sk: Doprava, en: Mobility}`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub title: BTreeMap<String, String>,
    /// The languages the portal is shown in, the first one the default, e.g. `[sk, en]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
    /// A host the operator listed for the Organization, e.g. `mobility.bratislava.sk`; absent:
    /// the slug address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// With a `host`: what the slug address does.
    #[serde(default)]
    pub slug: SlugAddress,
    /// Overrides of the Organization's appearance for this portal only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub appearance: Option<OrganizationAppearance>,
    /// The profiles its feeds serve beside EU DCAT-AP, e.g. `[dcat-ap-fi-2.0]`; `[]` for EU
    /// DCAT-AP alone. Absent, or naming only `dcat-ap-3.0.1`: the catalogue keeps what it serves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profiles: Option<Vec<String>>,
    /// Where its datasets come from (ODP-10).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<PortalSource>,
    /// Dataset names the portal does not list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden: Vec<String>,
    /// The portal's pages built from blocks (T-7418): `home` replaces the built-in home, each
    /// other page answers at `/page/{slug}`; absent: the built-in home alone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<crate::kinds::portal_page::PortalPage>,
}

impl Kind for PortalSpec {
    const KIND: &'static str = "Portal";
    const PLURAL: &'static str = "portals";
    const SCOPE: Scope = Scope::Organization;
    const PATH_TEMPLATE: &'static str = "portals/{name}.yaml";

    fn validate_spec(&self, meta: &ObjectMeta) -> Result<()> {
        names::validate_dns1123_label(&meta.name)?;
        self.validate()
    }
}

fn invalid(field: impl Into<String>, reason: impl Into<String>) -> Error {
    Error::Invalid {
        field: field.into(),
        reason: reason.into(),
    }
}

fn label(field: &str, value: &str) -> Result<()> {
    names::validate_dns1123_label(value).map_err(|_| {
        invalid(
            field,
            format!("`{value}` is not a DNS label, e.g. `mobility`"),
        )
    })
}

/// A bare lower-case host name of two labels or more: no scheme, port, path or wildcard.
pub fn is_portal_host(host: &str) -> bool {
    host.len() <= 253
        && host.contains('.')
        && host
            .split('.')
            .all(|l| names::validate_dns1123_label(l).is_ok())
}

impl PortalSpec {
    /// Whether this is the Organization's own portal at `/{org}/opendata/`.
    pub fn is_default(&self) -> bool {
        self.path == DEFAULT_PORTAL
    }

    /// The profiles it chooses, `[]` for none beside EU DCAT-AP. `None`: no choice is stated,
    /// as in a list of EU DCAT-AP alone, what editors wrote before a choice existed (T-7521).
    pub fn profile_choice(&self) -> Option<&[String]> {
        self.profiles
            .as_deref()
            .filter(|chosen| !matches!(chosen, [only] if only == BASE_PROFILE))
    }

    /// The manifest alone: names, host, languages, sources and their URLs.
    pub fn validate(&self) -> Result<()> {
        label("spec.organizationRef", &self.organization_ref)?;
        label("spec.path", &self.path)?;
        if let Some(host) = &self.host {
            if !is_portal_host(host) {
                return Err(invalid(
                    "spec.host",
                    format!("`{host}` is not a host name, e.g. `mobility.bratislava.sk`"),
                ));
            }
        }
        for lang in self.languages.iter().chain(self.title.keys()) {
            if lang.len() != 2 || !lang.bytes().all(|b| b.is_ascii_lowercase()) {
                return Err(Error::Locale(lang.clone()));
            }
        }
        if let Some(appearance) = &self.appearance {
            appearance.validate()?;
            appearance
                .display
                .validate("spec.appearance.display", &self.languages)?;
        }
        for name in &self.hidden {
            if name.is_empty() || name.len() > 100 {
                return Err(invalid(
                    "spec.hidden",
                    "a dataset name of 1 to 100 characters",
                ));
            }
        }
        if self.sources.len() > MAX_PORTAL_SOURCES {
            return Err(invalid(
                "spec.sources",
                format!("at most {MAX_PORTAL_SOURCES} sources"),
            ));
        }
        let mut seen = Vec::new();
        for (i, source) in self.sources.iter().enumerate() {
            source.validate(i)?;
            if seen.contains(&&source.name) {
                return Err(invalid(
                    format!("spec.sources[{i}].name"),
                    format!("`{}` names two sources of this portal", source.name),
                ));
            }
            seen.push(&source.name);
        }
        crate::kinds::portal_page::validate_pages(&self.pages)
    }
}

/// An `http(s)://` URL's host, lower-cased; `None` for any other scheme or no host.
pub fn http_url_host(url: &str) -> Option<String> {
    let lower = url.trim().to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.contains('@') {
        return None;
    }
    let host = authority.split(':').next()?;
    (!host.is_empty()).then(|| host.to_owned())
}

impl PortalSource {
    fn validate(&self, i: usize) -> Result<()> {
        let field = |f: &str| format!("spec.sources[{i}].{f}");
        label(&field("name"), &self.name)?;
        let harvested = self.kind != PortalSourceType::Endpoints;
        let narrows = !(self.projects.is_empty()
            && self.spaces.is_empty()
            && self.tags.is_empty()
            && self.names.is_empty());
        if !harvested {
            if self.url.is_some() || !self.allowed_hosts.is_empty() {
                return Err(invalid(
                    field("url"),
                    "an `endpoints` source lists this Organization's Endpoints and reads no URL",
                ));
            }
            return Ok(());
        }
        if narrows {
            return Err(invalid(
                field("type"),
                "projects, spaces, tags and names narrow an `endpoints` source only",
            ));
        }
        let url = self.url.as_deref().ok_or_else(|| {
            invalid(
                field("url"),
                "a harvested source names its catalogue, e.g. https://data.kosice.sk/catalog.ttl",
            )
        })?;
        // The harvest guard fetches https only: an http:// source would never run.
        let host = http_url_host(url)
            .filter(|_| {
                url.trim()
                    .get(..8)
                    .is_some_and(|s| s.eq_ignore_ascii_case("https://"))
            })
            .ok_or_else(|| {
                invalid(
                    field("url"),
                    "only an https:// address without user info is harvested, e.g. \
                     https://data.kosice.sk/catalog.ttl",
                )
            })?;
        if !self
            .allowed_hosts
            .iter()
            .any(|h| h.eq_ignore_ascii_case(&host))
        {
            return Err(invalid(
                field("allowedHosts"),
                format!("must list `{host}`, the url's own host"),
            ));
        }
        for h in &self.allowed_hosts {
            if !is_portal_host(h) {
                return Err(invalid(
                    field("allowedHosts"),
                    format!("`{h}` is not a host name"),
                ));
            }
        }
        if let Some(schedule) = &self.schedule {
            if !crate::cron::is_valid(schedule) {
                return Err(invalid(
                    field("schedule"),
                    "five cron fields, UTC, e.g. `23 3 * * *`",
                ));
            }
        }
        if let Some(gateway) = &self.gateway {
            if http_url_host(gateway).is_none() {
                return Err(invalid(field("gateway"), "an https:// or http:// address"));
            }
        }
        if let Some(file) = &self.converted_file {
            let plain = file.ends_with(".json")
                && file.len() <= 64
                && !file.starts_with('.')
                && file
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
            if !plain {
                return Err(invalid(
                    field("convertedFile"),
                    "a .json file name beside jc.toml, no directory, e.g. `od-datasets.json`",
                ));
            }
        }
        if let Some(base) = &self.datasets_base {
            if http_url_host(base).is_none() {
                return Err(invalid(
                    field("datasetsBase"),
                    "an https:// or http:// address, e.g. https://opendata.city.sk/",
                ));
            }
        }
        if let Some(budget) = self.budget {
            if budget.records == 0 || budget.bytes == 0 {
                return Err(invalid(field("budget"), "records and bytes above 0"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::ResourceEnvelope;

    fn parse(yaml: &str) -> Result<()> {
        ResourceEnvelope::<PortalSpec>::from_yaml(yaml)
            .map_err(|e| Error::Parse(e.to_string()))
            .and_then(|m| m.validate())
    }

    const HEAD: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Portal\nmetadata:\n  \
                        name: ba-mobility\n  namespace: org\n";

    #[test]
    fn a_portal_with_a_harvested_source_and_a_host_parses() {
        let yaml = format!(
            "{HEAD}spec:\n  organizationRef: bratislava\n  path: mobility\n  host: \
             mobility.bratislava.sk\n  slug: keep\n  title: {{sk: Doprava, en: Mobility}}\n  \
             languages: [sk, en]\n  sources:\n    - name: kosice\n      type: dcat-ap\n      \
             url: https://data.kosice.sk/catalog.ttl\n      allowedHosts: [data.kosice.sk]\n      \
             schedule: \"0 3 * * *\"\n      budget: {{records: 5000, bytes: 50000000}}\n    \
             - name: transport\n      type: endpoints\n      projects: [transport]\n"
        );
        parse(&yaml).expect("valid");
    }

    /// T-7743: a Portal's section headings keep to the languages it offers.
    #[test]
    fn a_display_heading_keeps_to_the_portal_languages() {
        let yaml = |lang: &str| {
            format!(
                "{HEAD}spec:\n  organizationRef: ba\n  path: x\n  languages: [sk, en]\n  \
                 appearance:\n    display:\n      dataset:\n        sections:\n          \
                 - name: about\n            heading: {{{lang}: O dátach}}\n"
            )
        };
        parse(&yaml("sk")).expect("a heading in an offered language");
        match parse(&yaml("de")) {
            Err(Error::Invalid { field, .. }) => {
                assert_eq!(
                    field,
                    "spec.appearance.display.dataset.sections[0].heading.de"
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_profile_choice_is_stated_or_absent() {
        let choice = |line: &str| {
            let yaml = format!("{HEAD}spec:\n  organizationRef: ba\n  path: x\n{line}");
            let held = ResourceEnvelope::<PortalSpec>::from_yaml(&yaml).expect("parses");
            held.spec.profile_choice().map(<[String]>::to_vec)
        };
        assert_eq!(choice(""), None);
        assert_eq!(
            choice("  profiles: [dcat-ap-3.0.1]\n"),
            None,
            "what editors wrote unasked"
        );
        assert_eq!(choice("  profiles: []\n"), Some(Vec::new()));
        assert_eq!(
            choice("  profiles: [dcat-ap-fi-2.0]\n"),
            Some(vec!["dcat-ap-fi-2.0".to_owned()])
        );
    }

    #[test]
    fn each_field_a_portal_gets_wrong_is_named() {
        for (spec, field) in [
            ("organizationRef: Bratislava\n  path: x", "spec.organizationRef"),
            ("organizationRef: ba\n  path: Mobility", "spec.path"),
            ("organizationRef: ba\n  path: x\n  host: https://x.sk", "spec.host"),
            ("organizationRef: ba\n  path: x\n  host: localhost", "spec.host"),
            (
                "organizationRef: ba\n  path: x\n  sources:\n    - {name: a, type: dcat-ap, url: \
                 'javascript:alert(1)', allowedHosts: [a.sk]}",
                "spec.sources[0].url",
            ),
            (
                "organizationRef: ba\n  path: x\n  sources:\n    - {name: a, type: dcat-ap, url: \
                 'http://a.sk/c.ttl', allowedHosts: [a.sk]}",
                "spec.sources[0].url",
            ),
            (
                "organizationRef: ba\n  path: x\n  sources:\n    - {name: a, type: dcat-ap, url: \
                 'https://a.sk/c.ttl', allowedHosts: [b.sk]}",
                "spec.sources[0].allowedHosts",
            ),
            (
                "organizationRef: ba\n  path: x\n  sources:\n    - {name: a, type: endpoints}\n    \
                 - {name: a, type: endpoints}",
                "spec.sources[1].name",
            ),
        ] {
            let err = parse(&format!("{HEAD}spec:\n  {spec}\n")).expect_err(spec);
            assert!(err.to_string().contains(field), "{spec}: {err}");
        }
        let err = parse(&format!(
            "{HEAD}spec:\n  organizationRef: ba\n  path: x\n  sources:\n    - {{name: a, type: \
             dcat-ap, url: 'https://a.sk', allowedHosts: [a.sk], secretRef: s}}\n"
        ))
        .expect_err("a source carries no credential");
        assert!(err.to_string().contains("secretRef"), "{err}");
    }

    #[test]
    fn each_harvested_type_has_its_wire_name_and_an_unknown_one_is_refused() {
        for (kind, wire) in [
            (PortalSourceType::ArcgisHub, "arcgis-hub"),
            (PortalSourceType::ArcgisRest, "arcgis-rest"),
            (PortalSourceType::OgcRecords, "ogc-records"),
        ] {
            assert_eq!(serde_json::to_value(kind).unwrap(), serde_json::json!(wire));
            parse(&format!(
                "{HEAD}spec:\n  organizationRef: ba\n  path: x\n  sources:\n    - {{name: a, \
                 type: {wire}, url: 'https://gis.a.sk/arcgis/rest/services', allowedHosts: \
                 [gis.a.sk]}}\n"
            ))
            .expect(wire);
        }
        let err = parse(&format!(
            "{HEAD}spec:\n  organizationRef: ba\n  path: x\n  sources:\n    - {{name: a, type: \
             arcgis-server, url: 'https://a.sk', allowedHosts: [a.sk]}}\n"
        ))
        .expect_err("unknown type");
        assert!(err.to_string().contains("arcgis-server"), "{err}");
    }

    #[test]
    fn a_converted_file_is_a_plain_json_name_beside_jc_toml() {
        let source = |extra: &str| {
            format!(
                "{HEAD}spec:\n  organizationRef: ba\n  path: default\n  sources:\n    - {{name: a, \
                 type: nkod-sparql, url: 'https://a.sk/sparql', allowedHosts: [a.sk]{extra}}}\n"
            )
        };
        parse(&source(
            ", convertedFile: od-datasets.json, datasetsBase: 'https://opendata.a.sk/', \
             enabled: false",
        ))
        .unwrap();
        for bad in [
            "../jc.toml",
            "/etc/x.json",
            "a/b.json",
            ".hidden.json",
            "jc.toml",
        ] {
            let err = parse(&source(&format!(", convertedFile: '{bad}'"))).expect_err(bad);
            assert!(err.to_string().contains("convertedFile"), "{bad}: {err}");
        }
        let err = parse(&source(", datasetsBase: 'ftp://a.sk/'")).expect_err("ftp");
        assert!(err.to_string().contains("datasetsBase"), "{err}");
        let spec: PortalSpec = serde_json::from_value(serde_json::json!({
            "organizationRef": "ba", "path": "default",
            "sources": [{"name": "a", "type": "dcat-ap", "url": "https://a.sk"}],
        }))
        .unwrap();
        assert!(
            spec.sources[0].enabled,
            "a source runs unless it says otherwise"
        );
        let back = serde_json::to_value(&spec).unwrap();
        assert!(back["sources"][0].get("enabled").is_none(), "{back}");
    }

    #[test]
    fn a_portal_checks_its_pages_through_the_manifest() {
        let yaml = |blocks: &str| {
            format!(
                "{HEAD}spec:\n  organizationRef: ba\n  path: x\n  pages:\n    - slug: home\n      \
                 rows:\n        - blocks: [{blocks}]\n"
            )
        };
        parse(&yaml("{type: numbers, items: [datasets]}")).expect("valid");
        let err = parse(&yaml("{type: chart, dataset: 'A B'}")).expect_err("bad name");
        assert!(
            err.to_string()
                .contains("spec.pages[0].rows[0].blocks[0].dataset"),
            "{err}"
        );
        let err = parse(&yaml("{type: text, markdown: {sk: x}, html: '<b>'}")).expect_err("html");
        assert!(err.to_string().contains("html"), "{err}");
    }

    #[test]
    fn only_http_urls_have_a_host() {
        assert_eq!(
            http_url_host("HTTPS://Data.sk:8443/x").as_deref(),
            Some("data.sk")
        );
        assert_eq!(http_url_host("http://a.sk?x").as_deref(), Some("a.sk"));
        for bad in [
            "javascript:alert(1)",
            "data:text/html,x",
            "ftp://a.sk",
            "https://u@a.sk/",
            "https:///x",
        ] {
            assert_eq!(http_url_host(bad), None, "{bad}");
        }
    }
}

#[cfg(test)]
mod edge_tests {
    use super::*;

    #[test]
    fn portal_is_default_and_title_locale() {
        let mut portal = PortalSpec {
            organization_ref: "org".into(),
            path: DEFAULT_PORTAL.into(),
            title: BTreeMap::new(),
            languages: Vec::new(),
            host: None,
            slug: SlugAddress::Redirect,
            appearance: None,
            profiles: None,
            sources: Vec::new(),
            hidden: Vec::new(),
            pages: Vec::new(),
        };
        assert!(portal.is_default());
        portal.path = "custom".into();
        assert!(!portal.is_default());

        portal.title.insert("INVALID_LOCALE".into(), "Title".into());
        let err = portal.validate().expect_err("bad locale in title rejected");
        assert!(matches!(err, Error::Locale(ref l) if l == "INVALID_LOCALE"));
    }

    #[test]
    fn portal_hidden_and_source_count_bounds() {
        let mut portal = PortalSpec {
            organization_ref: "org".into(),
            path: "mobility".into(),
            title: BTreeMap::new(),
            languages: Vec::new(),
            host: None,
            slug: SlugAddress::Redirect,
            appearance: None,
            profiles: None,
            sources: Vec::new(),
            hidden: vec!["".into()],
            pages: Vec::new(),
        };
        let err = portal
            .validate()
            .expect_err("empty hidden dataset name rejected");
        assert!(
            err.to_string()
                .contains("a dataset name of 1 to 100 characters"),
            "{err}"
        );

        portal.hidden = vec!["a".repeat(101)];
        let err = portal
            .validate()
            .expect_err("long hidden dataset name rejected");
        assert!(
            err.to_string()
                .contains("a dataset name of 1 to 100 characters"),
            "{err}"
        );

        portal.hidden.clear();
        for i in 0..=MAX_PORTAL_SOURCES {
            portal.sources.push(PortalSource {
                name: format!("src{i}"),
                kind: PortalSourceType::Endpoints,
                projects: Vec::new(),
                spaces: Vec::new(),
                tags: Vec::new(),
                names: Vec::new(),
                url: None,
                schedule: None,
                allowed_hosts: Vec::new(),
                budget: None,
                publisher: None,
                publisher_title: None,
                publisher_uri: None,
                publisher_description: None,
                spatial_uri: None,
                name_prefix: None,
                rewrite_hosts: BTreeMap::new(),
                skip: BTreeMap::new(),
                titles: BTreeMap::new(),
                spelling: BTreeMap::new(),
                require_column: BTreeMap::new(),
                gateway: None,
                converted_file: None,
                datasets_base: None,
                enabled: true,
            });
        }
        let err = portal.validate().expect_err("too many sources rejected");
        assert!(err.to_string().contains("at most 32 sources"), "{err}");
    }

    #[test]
    fn portal_source_type_and_url_rules() {
        let mut source = PortalSource {
            name: "endpoints-source".into(),
            kind: PortalSourceType::Endpoints,
            projects: Vec::new(),
            spaces: Vec::new(),
            tags: Vec::new(),
            names: Vec::new(),
            url: Some("https://example.com/feed".into()),
            schedule: None,
            allowed_hosts: Vec::new(),
            budget: None,
            publisher: None,
            publisher_title: None,
            publisher_uri: None,
            publisher_description: None,
            spatial_uri: None,
            name_prefix: None,
            rewrite_hosts: BTreeMap::new(),
            skip: BTreeMap::new(),
            titles: BTreeMap::new(),
            spelling: BTreeMap::new(),
            require_column: BTreeMap::new(),
            gateway: None,
            converted_file: None,
            datasets_base: None,
            enabled: true,
        };
        let err = source
            .validate(0)
            .expect_err("endpoints source with url rejected");
        assert!(
            err.to_string().contains(
                "an `endpoints` source lists this Organization's Endpoints and reads no URL"
            ),
            "{err}"
        );

        source.url = None;
        source.kind = PortalSourceType::DcatAp;
        source.projects = vec!["proj".into()];
        let err = source
            .validate(0)
            .expect_err("harvested source with projects rejected");
        assert!(
            err.to_string()
                .contains("projects, spaces, tags and names narrow an `endpoints` source only"),
            "{err}"
        );

        source.projects.clear();
        let err = source
            .validate(0)
            .expect_err("harvested source without url rejected");
        assert!(
            err.to_string().contains(
                "a harvested source names its catalogue, e.g. https://data.kosice.sk/catalog.ttl"
            ),
            "{err}"
        );
    }

    #[test]
    fn portal_source_hosts_schedule_gateway_and_budget() {
        let mut source = PortalSource {
            name: "harvested".into(),
            kind: PortalSourceType::DcatAp,
            projects: Vec::new(),
            spaces: Vec::new(),
            tags: Vec::new(),
            names: Vec::new(),
            url: Some("https://data.kosice.sk/catalog.ttl".into()),
            schedule: Some("invalid cron".into()),
            allowed_hosts: vec!["data.kosice.sk".into(), "bad host!".into()],
            budget: None,
            publisher: None,
            publisher_title: None,
            publisher_uri: None,
            publisher_description: None,
            spatial_uri: None,
            name_prefix: None,
            rewrite_hosts: BTreeMap::new(),
            skip: BTreeMap::new(),
            titles: BTreeMap::new(),
            spelling: BTreeMap::new(),
            require_column: BTreeMap::new(),
            gateway: None,
            converted_file: None,
            datasets_base: None,
            enabled: true,
        };
        let err = source.validate(0).expect_err("invalid host rejected");
        assert!(err.to_string().contains("is not a host name"), "{err}");

        source.allowed_hosts = vec!["data.kosice.sk".into()];
        let err = source.validate(0).expect_err("invalid schedule rejected");
        assert!(
            err.to_string()
                .contains("five cron fields, UTC, e.g. `23 3 * * *`"),
            "{err}"
        );

        source.schedule = Some("0 3 * * *".into());
        source.gateway = Some("not an http url".into());
        let err = source.validate(0).expect_err("invalid gateway rejected");
        assert!(
            err.to_string().contains("an https:// or http:// address"),
            "{err}"
        );

        source.gateway = None;
        source.budget = Some(HarvestBudget {
            records: 0,
            bytes: 100,
        });
        let err = source
            .validate(0)
            .expect_err("zero records budget rejected");
        assert!(
            err.to_string().contains("records and bytes above 0"),
            "{err}"
        );

        source.budget = Some(HarvestBudget {
            records: 100,
            bytes: 0,
        });
        let err = source.validate(0).expect_err("zero bytes budget rejected");
        assert!(
            err.to_string().contains("records and bytes above 0"),
            "{err}"
        );
    }
    #[test]
    fn a_portal_with_a_hidden_dataset_validates() {
        let portal = PortalSpec {
            organization_ref: "org".into(),
            path: "mobility".into(),
            title: BTreeMap::new(),
            languages: Vec::new(),
            host: None,
            slug: SlugAddress::Redirect,
            appearance: None,
            profiles: None,
            sources: Vec::new(),
            hidden: vec!["old-buses".into()],
            pages: Vec::new(),
        };
        portal
            .validate()
            .expect("a hidden name of 1 to 100 characters");
    }

    #[test]
    fn a_harvested_source_with_an_https_gateway_validates() {
        let source = PortalSource {
            name: "harvested".into(),
            kind: PortalSourceType::DcatAp,
            projects: Vec::new(),
            spaces: Vec::new(),
            tags: Vec::new(),
            names: Vec::new(),
            url: Some("https://data.kosice.sk/catalog.ttl".into()),
            schedule: None,
            allowed_hosts: vec!["data.kosice.sk".into()],
            budget: None,
            publisher: None,
            publisher_title: None,
            publisher_uri: None,
            publisher_description: None,
            spatial_uri: None,
            name_prefix: None,
            rewrite_hosts: BTreeMap::new(),
            skip: BTreeMap::new(),
            titles: BTreeMap::new(),
            spelling: BTreeMap::new(),
            require_column: BTreeMap::new(),
            gateway: Some("https://gateway.kosice.sk/".into()),
            converted_file: None,
            datasets_base: None,
            enabled: true,
        };
        source.validate(0).expect("an https gateway");
    }
}
