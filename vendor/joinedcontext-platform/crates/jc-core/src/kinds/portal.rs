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
    /// A harvested catalogue's address, `https://` or `http://`.
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
    /// The DCAT-AP profiles its feeds serve, e.g. `[dcat-ap-3.0.1]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<String>,
    /// Where its datasets come from (ODP-10).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<PortalSource>,
    /// Dataset names the portal does not list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden: Vec<String>,
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
    names::validate_dns1123_label(value)
        .map_err(|_| invalid(field, format!("`{value}` is not a DNS label, e.g. `mobility`")))
}

/// A bare lower-case host name of two labels or more: no scheme, port, path or wildcard.
pub fn is_portal_host(host: &str) -> bool {
    host.len() <= 253
        && host.contains('.')
        && host.split('.').all(|l| names::validate_dns1123_label(l).is_ok())
}

impl PortalSpec {
    /// Whether this is the Organization's own portal at `/{org}/opendata/`.
    pub fn is_default(&self) -> bool {
        self.path == DEFAULT_PORTAL
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
        }
        for name in &self.hidden {
            if name.is_empty() || name.len() > 100 {
                return Err(invalid("spec.hidden", "a dataset name of 1 to 100 characters"));
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
        Ok(())
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
        let host = http_url_host(url).ok_or_else(|| {
            invalid(
                field("url"),
                "only an https:// or http:// address without user info is harvested",
            )
        })?;
        if !self.allowed_hosts.iter().any(|h| h.eq_ignore_ascii_case(&host)) {
            return Err(invalid(
                field("allowedHosts"),
                format!("must list `{host}`, the url's own host"),
            ));
        }
        for h in &self.allowed_hosts {
            if !is_portal_host(h) {
                return Err(invalid(field("allowedHosts"), format!("`{h}` is not a host name")));
            }
        }
        if let Some(schedule) = &self.schedule {
            if !crate::cron::is_valid(schedule) {
                return Err(invalid(field("schedule"), "five cron fields, UTC, e.g. `23 3 * * *`"));
            }
        }
        if let Some(gateway) = &self.gateway {
            if http_url_host(gateway).is_none() {
                return Err(invalid(field("gateway"), "an https:// or http:// address"));
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
    fn only_http_urls_have_a_host() {
        assert_eq!(http_url_host("HTTPS://Data.sk:8443/x").as_deref(), Some("data.sk"));
        assert_eq!(http_url_host("http://a.sk?x").as_deref(), Some("a.sk"));
        for bad in ["javascript:alert(1)", "data:text/html,x", "ftp://a.sk", "https://u@a.sk/", "https:///x"] {
            assert_eq!(http_url_host(bad), None, "{bad}");
        }
    }
}
