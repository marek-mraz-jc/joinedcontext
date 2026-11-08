#![forbid(unsafe_code)]
//! Domain records of `dataroom`.

use crate::error::{DataroomError, Result};
use chrono::{DateTime, NaiveDate, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;
use uuid::Uuid;

/// Multilingual text map keyed by ISO 639-1 language code (`sk`, `cs`, `en`, ...).
pub type Texts = BTreeMap<String, String>;

/// Helper to read a string in a preferred language, falling back to `en`, `sk`, `cs` or first.
pub fn text_in_lang<'a>(texts: &'a Texts, preferred: Option<&str>) -> Option<&'a str> {
    if let Some(lang) = preferred {
        if let Some(val) = texts.get(lang).filter(|v| !v.is_empty()) {
            return Some(val.as_str());
        }
    }
    for fallback in ["en", "sk", "cs"] {
        if let Some(val) = texts.get(fallback).filter(|v| !v.is_empty()) {
            return Some(val.as_str());
        }
    }
    texts.values().find(|v| !v.is_empty()).map(String::as_str)
}

/// Verified Organization identifier (UUID, no `Default` and no `From<&str>` per research-tenancy §2.1).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct OrgId(Uuid);

impl OrgId {
    /// Creates an `OrgId` from a verified `Uuid`.
    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    /// Parses a verified UUID string into an `OrgId`.
    pub fn parse_verified(text: &str) -> Result<Self> {
        let u = Uuid::parse_str(text)
            .map_err(|e| DataroomError::field_error("org_id", format!("invalid UUID: {e}")))?;
        if u.is_nil() {
            return Err(DataroomError::field_error(
                "org_id",
                "nil UUID is not allowed",
            ));
        }
        Ok(Self(u))
    }

    /// Returns the inner `Uuid`.
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl fmt::Display for OrgId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Verified Catalogue instance identifier (UUID, no `Default` and no `From<&str>`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct CatalogueId(Uuid);

impl CatalogueId {
    /// Creates a `CatalogueId` from a verified `Uuid`.
    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    /// Parses a verified UUID string into a `CatalogueId`.
    pub fn parse_verified(text: &str) -> Result<Self> {
        let u = Uuid::parse_str(text).map_err(|e| {
            DataroomError::field_error("catalogue_id", format!("invalid UUID: {e}"))
        })?;
        if u.is_nil() {
            return Err(DataroomError::field_error(
                "catalogue_id",
                "nil UUID is not allowed",
            ));
        }
        Ok(Self(u))
    }

    /// Returns the inner `Uuid`.
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl fmt::Display for CatalogueId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Visibility of a dataset (00-contract.md).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    /// Published in open DCAT feeds and visible to anonymous callers.
    Public,
    /// Visible inside the organization to members with read permission.
    Internal,
    /// Visible only to principals holding a specific grant on the dataset or publisher.
    Restricted,
}

impl Visibility {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Internal => "internal",
            Self::Restricted => "restricted",
        }
    }
}

/// Lifecycle state of a dataset or publisher.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum RecordState {
    Draft,
    Active,
    Withdrawn,
}

impl RecordState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Active => "active",
            Self::Withdrawn => "withdrawn",
        }
    }

    /// CKAN state string (`active`, `draft`, `deleted`).
    pub const fn ckan_state(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Active => "active",
            Self::Withdrawn => "deleted",
        }
    }
}

/// Pinned DCAT-AP profile version on a catalogue (C17).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProfilePin {
    pub profile_id: String,
    pub version: String,
}

/// Theme tokens for server-rendered UI per catalogue (C12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThemeTokens {
    pub instance_name: String,
    pub organisation: String,
    pub primary: String,
    pub secondary: String,
    pub accent: String,
    pub background: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tagline: Option<String>,
}

impl Default for ThemeTokens {
    fn default() -> Self {
        Self {
            instance_name: "dataroom".into(),
            organisation: "Open Data".into(),
            primary: "#0000bf".into(),
            secondary: "#0072c6".into(),
            accent: "#ffe977".into(),
            background: "#ffffff".into(),
            text: "#1a1a1a".into(),
            logo: None,
            tagline: None,
        }
    }
}

/// Kind of contact point (`vcard:Organization` or `vcard:Individual`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
pub enum ContactKind {
    #[default]
    Organization,
    Individual,
}

/// Contact point (`dcat:contactPoint`, `vcard:Kind`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ContactPoint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(default)]
    pub kind: ContactKind,
    pub name: Texts,
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
}

/// One catalogue instance (`cat_catalogue`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Catalogue {
    pub org_id: OrgId,
    pub id: CatalogueId,
    pub name: String,
    pub host: String,
    pub title: Texts,
    pub description: Texts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    pub default_language: String,
    pub languages: Vec<String>,
    pub main_profile: String,
    pub active_profiles: Vec<ProfilePin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_licence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher_name: Option<Texts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_point: Option<ContactPoint>,
    #[serde(default)]
    pub theme: ThemeTokens,
    /// Optional per-catalogue branding override JSON (`global.branding` shape, B1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branding: Option<Value>,
    /// Package names hidden by the publish gate (`jc_hide`, T-4318, D2, E4): `{name: reason}`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub hidden_datasets: BTreeMap<String, String>,
    #[serde(default = "default_true")]
    pub allow_cross_org_harvest: bool,
    #[serde(default)]
    pub allowed_outbound_hosts: Vec<String>,
    #[serde(default)]
    pub applicable_legislation: Vec<String>,
    pub modified: DateTime<Utc>,
}

fn default_true() -> bool {
    true
}

impl Catalogue {
    /// Base URL for this catalogue (`https://{host}`).
    pub fn base_url(&self) -> String {
        if self.host.starts_with("http://") || self.host.starts_with("https://") {
            self.host.trim_end_matches('/').to_owned()
        } else {
            format!("https://{}", self.host.trim_end_matches('/'))
        }
    }

    /// Computes the effective [`crate::branding::BrandingBlock`] for this catalogue by layering
    /// the catalogue's `theme` and `branding` override on top of `installation_branding` (B1, G4).
    pub fn effective_branding(
        &self,
        installation_branding: Option<&crate::branding::BrandingBlock>,
    ) -> crate::branding::BrandingBlock {
        let mut block = installation_branding.cloned().unwrap_or_default();
        // Apply catalogue languages and title if not already overridden
        if !self.default_language.is_empty() {
            block.languages.default = self.default_language.clone();
        }
        if !self.languages.is_empty() {
            block.languages.offered = self.languages.clone();
        }
        for (lang, t) in &self.title {
            if !t.is_empty() {
                block.site_title.insert(lang.clone(), t.clone());
            }
        }
        // Apply ThemeTokens when different from default or explicitly set
        let def_theme = ThemeTokens::default();
        if self.theme != def_theme {
            if !self.theme.instance_name.is_empty() && self.theme.instance_name != "dataroom" {
                block.instance_name = self.theme.instance_name.clone();
            }
            if !self.theme.organisation.is_empty() && self.theme.organisation != "Open Data" {
                block.organisation = self.theme.organisation.clone();
            }
            if crate::branding::is_valid_hex_colour(&self.theme.primary) {
                block.colours.primary = self.theme.primary.clone();
            }
            if crate::branding::is_valid_hex_colour(&self.theme.secondary) {
                block.colours.secondary = self.theme.secondary.clone();
            }
            if crate::branding::is_valid_hex_colour(&self.theme.accent) {
                block.colours.accent = self.theme.accent.clone();
            }
            if crate::branding::is_valid_hex_colour(&self.theme.background) {
                block.colours.background = self.theme.background.clone();
            }
            if crate::branding::is_valid_hex_colour(&self.theme.text) {
                block.colours.text = self.theme.text.clone();
            }
            if let Some(ref l) = self.theme.logo {
                block.logo = l.clone();
            }
            if let Some(ref tl) = self.theme.tagline {
                block.tagline = tl.clone();
            }
            block.recompute_derived();
        }
        if let Some(ref ov) = self.branding {
            block.apply_override_json(ov);
        }
        block
    }
}

/// A managed Publisher record under a catalogue (`cat_publisher`, C16).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Publisher {
    pub org_id: OrgId,
    pub catalogue_id: CatalogueId,
    /// Left empty by a caller creating a publisher; the store assigns `pub-{name}`.
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub title: Texts,
    #[serde(default)]
    pub description: Texts,
    /// Registry IRI (e.g. OVM IRI in CZ or legal-subject IČO IRI in SK).
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_point: Option<ContactPoint>,
    pub state: RecordState,
    /// When merged into another publisher, holds the target publisher `id` (C16).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_into: Option<String>,
    /// Person or group IDs granted editor rights on this publisher's datasets (C16).
    #[serde(default)]
    pub editor_grants: Vec<String>,
    /// Badge initials for `_badge` when `logo_url` is unset (B1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge_initials: Option<String>,
    /// Badge background hex colour for `_badge` when `logo_url` is unset (B1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge_colour: Option<String>,
    /// Last `CkanInstance` name that declared `title` for this organization (G2: one instance
    /// never renames another's organization).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_by_instance: Option<String>,
    pub created: DateTime<Utc>,
    pub modified: DateTime<Utc>,
}

/// Agent summary embedded on a dataset or service (`foaf:Agent`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    pub name: Texts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
}

/// Temporal period (`dct:PeriodOfTime`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TemporalPeriod {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<NaiveDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<NaiveDate>,
}

/// National terms-of-use specification (`pu:Specifikace` in DCAT-AP-CZ, `leg:TermsOfUse` in DCAT-AP-SK).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TermsOfUse {
    /// `pu:autorské-dílo` / `leg:authorsWorkType`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authors_work: Option<String>,
    /// `pu:autor` / `leg:authorName`
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub author_name: Texts,
    /// `pu:databáze-jako-autorské-dílo` / `leg:originalDatabaseType`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database_as_work: Option<String>,
    /// `pu:autor-databáze` / `leg:originalDatabaseAuthorName`
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub database_author_name: Texts,
    /// `pu:databáze-chráněná-zvláštními-právy` / `leg:databaseProtectedBySpecialRightsType`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sui_generis_database: Option<String>,
    /// `pu:osobní-údaje` / `leg:personalDataContainmentType`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub personal_data: Option<String>,
}

/// DataService (`dcat:DataService`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DataService {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    pub title: Texts,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub description: Texts,
    pub endpoint_urls: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub endpoint_descriptions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub serves_datasets: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conforms_to: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub applicable_legislation: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub licence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_rights: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_point: Option<ContactPoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<AgentDetails>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub documentation: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub formats: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub themes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
}

/// A resource view (`cat_view`, e.g. `jc_visualization`, `datatables_view`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResourceView {
    pub id: String,
    pub resource_id: String,
    pub view_type: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub order: i32,
    #[serde(default)]
    pub config: BTreeMap<String, Value>,
}

/// A distribution (`dcat:Distribution`, CKAN resource, `cat_distribution`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Distribution {
    pub id: String,
    pub dataset_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    pub title: Texts,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub description: Texts,
    pub access_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compress_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash_algorithm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub licence: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rights: Texts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub availability: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_type: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conforms_to: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub documentation: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub applicable_legislation: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temporal_resolution: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spatial_resolution_in_meters: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terms_of_use: Option<TermsOfUse>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub access_services: Vec<DataService>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issued: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extras: BTreeMap<String, Value>,
}

/// A dataset or dataset series (`dcat:Dataset`, `dcat:DatasetSeries`, `cat_dataset`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Dataset {
    pub org_id: OrgId,
    pub catalogue_id: CatalogueId,
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(default = "default_dataset_type")]
    pub dataset_type: String,
    pub title: Texts,
    pub description: Texts,
    pub publisher_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<AgentDetails>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator: Option<AgentDetails>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_point: Option<ContactPoint>,
    pub visibility: Visibility,
    pub state: RecordState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub licence_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub licence_url: Option<String>,
    #[serde(default)]
    pub themes: Vec<String>,
    #[serde(default)]
    pub eurovoc_themes: Vec<String>,
    #[serde(default)]
    pub keywords: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub spatial: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spatial_resolution_in_meters: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temporal: Option<TemporalPeriod>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temporal_resolution: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landing_page: Option<String>,
    #[serde(default)]
    pub documentation: Vec<String>,
    #[serde(default)]
    pub conforms_to: Vec<String>,
    #[serde(default)]
    pub applicable_legislation: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_series: Option<String>,
    #[serde(default)]
    pub relations: Vec<String>,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default)]
    pub version_notes: Texts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_rights: Option<String>,
    #[serde(default)]
    pub provenance: Texts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dcat_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    #[serde(default)]
    pub alternate_identifiers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harvest_source_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harvest_source_item_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harvest_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_shacl_shapes: Option<String>,
    #[serde(default)]
    pub distributions: Vec<Distribution>,
    #[serde(default)]
    pub extras: BTreeMap<String, String>,
    /// Principals granted read access when `visibility == Restricted`.
    #[serde(default)]
    pub restricted_grants: Vec<String>,
    pub issued: DateTime<Utc>,
    pub modified: DateTime<Utc>,
}

fn default_dataset_type() -> String {
    "dataset".into()
}

impl Dataset {
    /// Canonical dataset IRI for DCAT serializations.
    pub fn canonical_uri(&self, catalogue_base: &str) -> String {
        if let Some(uri) = &self.uri {
            if !uri.trim().is_empty() {
                return uri.trim().to_owned();
            }
        }
        if let Some(uri) = self
            .extras
            .get("uri")
            .or_else(|| self.extras.get("source_uri"))
        {
            if !uri.trim().is_empty() {
                return uri.trim().to_owned();
            }
        }
        format!(
            "{}/dataset/{}",
            catalogue_base.trim_end_matches('/'),
            self.name
        )
    }

    /// Validates slug, tags and required fields.
    pub fn validate_basic(&self) -> Result<()> {
        validate_slug("name", &self.name)?;
        if self.title.values().all(|v| v.trim().is_empty()) {
            return Err(DataroomError::field_error(
                "title",
                "at least one non-empty language title is required",
            ));
        }
        if self.publisher_id.trim().is_empty() {
            return Err(DataroomError::field_error(
                "owner_org",
                "Organization does not exist",
            ));
        }
        for tag in &self.tags {
            let count = tag.chars().count();
            if !(2..=100).contains(&count) {
                return Err(DataroomError::field_error(
                    "tags",
                    format!("tag '{tag}' must be between 2 and 100 characters"),
                ));
            }
            if !tag
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'))
            {
                return Err(DataroomError::field_error(
                    "tags",
                    format!("tag '{tag}' contains invalid characters"),
                ));
            }
        }
        Ok(())
    }
}

/// Validates a CKAN-compatible slug (`2..=100` chars of `a-z`, `0-9`, `-`, `_`).
pub fn validate_slug(field: &str, value: &str) -> Result<()> {
    let ok = (2..=100).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        && !value.starts_with(['-', '_'])
        && !value.ends_with(['-', '_']);
    if !ok {
        return Err(DataroomError::field_error(
            field,
            "must be 2 to 100 characters of lowercase letters, digits, '-' and '_'",
        ));
    }
    Ok(())
}

/// Kind of harvest source (C9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum HarvestSourceKind {
    DcatAp,
    NkodSparql,
    CkanAction,
    ArcGisHub,
}

impl HarvestSourceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DcatAp => "dcat-ap",
            Self::NkodSparql => "nkod-sparql",
            Self::CkanAction => "ckan-action",
            Self::ArcGisHub => "arcgis-hub",
        }
    }
}

/// Harvest source configuration (`cat_harvest_source`, C9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarvestSource {
    pub org_id: OrgId,
    pub catalogue_id: CatalogueId,
    /// Left empty by a caller creating a source; the store assigns `hs-{name}`.
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub kind: HarvestSourceKind,
    pub url: String,
    pub publisher_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_publisher_iri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spatial_uri: Option<String>,
    #[serde(default)]
    pub name_prefix: String,
    pub allowed_hosts: Vec<String>,
    #[serde(default)]
    pub rewrite_hosts: BTreeMap<String, String>,
    #[serde(default)]
    pub skip_items: BTreeMap<String, String>,
    #[serde(default)]
    pub title_overrides: BTreeMap<String, String>,
    #[serde(default)]
    pub spelling_fixes: BTreeMap<String, String>,
    #[serde(default)]
    pub require_column: BTreeMap<String, String>,
    #[serde(default = "default_schedule")]
    pub schedule: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub modified: DateTime<Utc>,
}

fn default_schedule() -> String {
    "23 3 * * *".into()
}

/// State of a harvest run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum HarvestRunState {
    Running,
    Succeeded,
    Failed,
}

impl HarvestRunState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }
}

/// One item skipped during a harvest run, with its reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarvestSkippedItem {
    pub item_id: String,
    pub title: String,
    pub reason: String,
}

/// One structured log line of a harvest run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarvestLogLine {
    pub at: DateTime<Utc>,
    pub level: String,
    pub fingerprint: String,
    pub message: String,
}

/// One execution of a harvest source (`cat_harvest_run`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarvestRun {
    pub org_id: OrgId,
    pub catalogue_id: CatalogueId,
    pub id: String,
    pub source_id: String,
    pub state: HarvestRunState,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    pub created_count: usize,
    pub updated_count: usize,
    pub unchanged_count: usize,
    pub withdrawn_count: usize,
    #[serde(default)]
    pub skipped: Vec<HarvestSkippedItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub logs: Vec<HarvestLogLine>,
}

/// Activity audit log entry (`cat_activity`, C13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActivityRow {
    pub org_id: OrgId,
    pub catalogue_id: CatalogueId,
    pub id: String,
    pub at: DateTime<Utc>,
    pub actor: String,
    pub operation: String,
    pub object_type: String,
    pub object_id: String,
    pub summary: String,
    #[serde(default)]
    pub details: Value,
}

/// SHACL severity (`sh:Info`, `sh:Warning`, `sh:Violation`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub enum ShaclSeverity {
    Info,
    Warning,
    Violation,
}

impl ShaclSeverity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "Info",
            Self::Warning => "Warning",
            Self::Violation => "Violation",
        }
    }
}

/// A single SHACL validation finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ShaclFinding {
    pub focus_node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub source_constraint_component: String,
    pub source_shape: String,
    pub severity: ShaclSeverity,
    pub message: Texts,
}

/// Stored SHACL validation report for one `(dataset, profile, version)` (C8, C17).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ShaclValidationReport {
    pub org_id: OrgId,
    pub catalogue_id: CatalogueId,
    pub id: String,
    pub dataset_id: String,
    pub profile_id: String,
    pub profile_version: String,
    pub conforms: bool,
    pub violation_count: usize,
    pub warning_count: usize,
    pub findings: Vec<ShaclFinding>,
    pub checked_at: DateTime<Utc>,
}

/// Form field layout derived from a profile's SHACL shapes (C17).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FormFieldLayout {
    pub path: String,
    pub field_name: String,
    pub name: Texts,
    pub description: Texts,
    pub group: Option<String>,
    pub order: i32,
    pub required: bool,
    pub max_count: Option<usize>,
    pub node_kind: Option<String>,
    pub datatype: Option<String>,
    pub class_iri: Option<String>,
    pub allowed_values: Vec<String>,
}

/// Versioned DCAT-AP profile record (C17).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRecord {
    pub id: String,
    pub version: String,
    pub uri: String,
    pub title: Texts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_profile: Option<String>,
    pub shapes_turtle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jsonld_context: Option<String>,
    #[serde(default)]
    pub required_languages: Vec<String>,
    pub built_in: bool,
    #[serde(default)]
    pub form_fields: Vec<FormFieldLayout>,
}

/// Dry-run report produced when uploading or switching a profile version (C17).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDryRunReport {
    pub profile_id: String,
    pub version: String,
    pub total_datasets: usize,
    pub conforming_datasets: usize,
    pub failing_datasets: usize,
    pub newly_failing_datasets: Vec<String>,
    pub dataset_reports: Vec<ShaclValidationReport>,
}

/// Static Markdown page per catalogue and locale (`cat_page`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StaticPage {
    pub org_id: OrgId,
    pub catalogue_id: CatalogueId,
    pub slug: String,
    pub lang: String,
    pub title: String,
    pub markdown: String,
    pub modified: DateTime<Utc>,
}

/// Installed WASM or built-in plugin record (`cat_plugin`, C10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CataloguePlugin {
    pub org_id: OrgId,
    pub catalogue_id: CatalogueId,
    pub name: String,
    pub version: String,
    pub digest: String,
    pub enabled: bool,
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    #[serde(default)]
    pub config: Value,
    pub modified: DateTime<Utc>,
}
