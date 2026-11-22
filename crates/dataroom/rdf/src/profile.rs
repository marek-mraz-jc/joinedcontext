//! Catalogue publication profiles as data (T-7521): every directory of `crates/dataroom/profiles`
//! holds a `profile.json` beside or pointing at its shapes and contexts, embedded by `build.rs`
//! and parsed once. A country or a standard is a directory; this module knows what a manifest
//! may say and nothing of any country. A profile names its bases: shapes and properties add up,
//! every other member is the profile's own, else the first base's that has one.

use crate::dcat::RdfFormat;
use dataroom_core::{DataroomError, Result, Texts};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

include!(concat!(env!("OUT_DIR"), "/profiles.rs"));

/// The profile every catalogue serves: EU DCAT-AP.
pub const BASE_PROFILE: &str = "dcat-ap-3.0.1";

/// A row a profile's specification requires and its published shapes do not check.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Required {
    pub target: String,
    pub path: String,
    /// The row's own IRI in the specification.
    pub source: String,
}

/// What the profile asks of the catalogue itself.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CatalogueRules {
    /// A declared publisher IRI, or no feed.
    pub publisher: bool,
    /// The register's prefixes such an IRI has when the Organization is asked to choose the profile.
    #[serde(default)]
    pub publisher_iri_prefixes: Vec<String>,
}

/// The terms-of-use node: the predicate that links it, its class and one predicate per member.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Terms {
    pub link: String,
    pub class: String,
    pub authors_work: String,
    pub author_name: String,
    pub database_as_work: String,
    pub database_author_name: String,
    pub sui_generis_database: String,
    pub personal_data: String,
}

/// The code list of the personal-data statement (CMP-08).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PersonalData {
    pub contains: String,
    pub does_not_contain: String,
}

/// The subject an additional property is written on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Subject {
    Dataset,
    /// Every distribution of the dataset that declares it.
    Distribution,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ValueKind {
    Iri,
    Literal,
}

/// A property written from a named extra of the record, only where the record declares it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Property {
    pub extra: String,
    pub subject: Subject,
    pub predicate: String,
    pub kind: ValueKind,
    /// The class of an IRI value.
    pub class: Option<String>,
    /// The code list: when given, only its IRIs are written, each with its published label.
    #[serde(default)]
    pub values: BTreeMap<String, Texts>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CatalogueMember {
    Title,
    Description,
    Contact,
    Homepage,
    Publisher,
    AccessPoint,
    Datasets,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DatasetMember {
    Title,
    Description,
    Publisher,
    Themes,
    Keywords,
    /// Always written, the tags where no keyword is declared.
    KeywordsOrTags,
    Frequency,
    Spatial,
    LandingPage,
    Contact,
    Distributions,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DistributionMember {
    Terms,
    AccessUrl,
    DownloadUrl,
    Format,
    MediaType,
    Title,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TermsMember {
    AuthorsWork,
    DatabaseAsWork,
    SuiGenerisDatabase,
    PersonalData,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Contact {
    pub organization: String,
    pub individual: String,
    /// The keys the name is written under.
    pub name: Vec<String>,
    pub email: String,
}

/// A compact node: its class and its members in the order they are written, each with its key.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Node<M> {
    pub class: String,
    pub members: Vec<(M, String)>,
}

/// A compact document: a node under a context.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Document<M> {
    pub context: String,
    pub class: String,
    pub contact: Contact,
    pub members: Vec<(M, String)>,
}

/// The key dictionary of a compact JSON-LD serialisation.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Compact {
    pub iri: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub catalogue: Document<CatalogueMember>,
    pub dataset: Document<DatasetMember>,
    pub distribution: Node<DistributionMember>,
    pub terms: Node<TermsMember>,
}

/// How the one graph writer and the one compact JSON-LD writer render a profile.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase", default)]
pub struct Render {
    pub identifier: Option<bool>,
    pub issued_modified: Option<bool>,
    /// `dct:license` with the licence's IRI on a distribution.
    pub licence: Option<bool>,
    /// The dataset's legislation repeated on its distributions and services.
    pub inherit_legislation: Option<bool>,
    pub terms: Option<Terms>,
    pub personal_data: Option<PersonalData>,
    /// The class the shapes want stated on every IRI value of a predicate, by predicate.
    pub value_classes: Option<BTreeMap<String, String>>,
    /// The only checksum algorithms the shapes admit; another leaves no checksum node.
    pub checksum_algorithms: Option<Vec<String>>,
    pub json_ld: Option<Compact>,
}

/// A remote `@context` that is read from a file of the profile, never fetched.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Context {
    #[serde(default)]
    pub prefixes: Vec<String>,
    #[serde(default)]
    pub suffixes: Vec<String>,
    pub file: String,
    /// Terms the published file uses and leaves out.
    #[serde(default)]
    pub add: serde_json::Map<String, serde_json::Value>,
}

/// `profile.json`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase", default)]
pub struct Profile {
    pub id: String,
    pub version: String,
    pub uri: String,
    /// Where the shapes come from, their version and licence.
    pub source: String,
    pub title: Texts,
    /// ISO 3166-1 alpha-2, none for a profile of no one country.
    pub country: Option<String>,
    pub languages: Vec<String>,
    /// What else a reader may ask for it by, compared whole and whatever the case.
    pub aliases: Vec<String>,
    pub bases: Vec<String>,
    /// Offered to an Organization choosing its catalogue's profiles.
    pub selectable: bool,
    /// Served at `/profiles/{id}/catalog.{suffix}`; the first base's when empty.
    pub serialisations: Vec<RdfFormat>,
    pub shapes: Vec<String>,
    pub required: Vec<Required>,
    pub catalogue: Option<CatalogueRules>,
    pub render: Render,
    pub properties: Vec<Property>,
    pub contexts: Vec<Context>,
    #[serde(skip)]
    files: &'static [(&'static str, &'static str)],
}

impl Profile {
    /// An embedded file the manifest names.
    pub fn file(&self, path: &str) -> &'static str {
        self.files
            .iter()
            .find(|(name, _)| *name == path)
            .map_or("", |(_, text)| text)
    }

    /// The profile's own shapes: its files and a shape for each required row.
    pub fn shapes_turtle(&self) -> String {
        let mut parts: Vec<String> = self
            .shapes
            .iter()
            .map(|f| self.file(f).to_owned())
            .collect();
        if !self.required.is_empty() {
            let sh = "http://www.w3.org/ns/shacl#";
            let targets: BTreeSet<&str> = self.required.iter().map(|r| r.target.as_str()).collect();
            let mut rows = String::new();
            for (n, target) in targets.into_iter().enumerate() {
                let node = format!("<urn:jc:profile:{}:required:{n}>", self.id);
                rows.push_str(&format!(
                    "{node} a <{sh}NodeShape> ; <{sh}targetClass> <{target}> .\n"
                ));
                for row in self.required.iter().filter(|r| r.target == target) {
                    rows.push_str(&format!(
                        "{node} <{sh}property> [ <{sh}path> <{}> ; <{sh}minCount> 1 ; \
                         <http://www.w3.org/2000/01/rdf-schema#seeAlso> <{}> ] .\n",
                        row.path, row.source
                    ));
                }
            }
            parts.push(rows);
        }
        parts.join("\n")
    }

    fn inherit(&mut self, base: &Self) {
        let (own, of_base) = (&mut self.render, &base.render);
        macro_rules! or_base {
            ($($member:ident),*) => {$(
                if own.$member.is_none() {
                    own.$member.clone_from(&of_base.$member);
                }
            )*};
        }
        or_base!(
            identifier,
            issued_modified,
            licence,
            inherit_legislation,
            terms,
            personal_data,
            value_classes,
            checksum_algorithms,
            json_ld
        );
        if self.catalogue.is_none() {
            self.catalogue.clone_from(&base.catalogue);
        }
        if self.serialisations.is_empty() {
            self.serialisations.clone_from(&base.serialisations);
        }
        for property in &base.properties {
            let held = self
                .properties
                .iter()
                .any(|p| p.predicate == property.predicate && p.subject == property.subject);
            if !held {
                self.properties.push(property.clone());
            }
        }
    }
}

/// An IRI that can stand between `<` and `>` in Turtle.
fn is_iri(text: &str) -> bool {
    text.contains(':')
        && !text
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "<>\"{}|\\^`".contains(c))
}

/// Parses and resolves manifests `(directory, text, files)`; the error names the directory.
fn load(sources: &[Embedded]) -> std::result::Result<Vec<Profile>, String> {
    let mut read = Vec::new();
    let mut names = BTreeSet::new();
    for (dir, text, files) in sources {
        let bad = |what: &str| format!("profile {dir}: {what}");
        let mut profile: Profile = serde_json::from_str(text).map_err(|e| bad(&e.to_string()))?;
        profile.files = *files;
        if profile.id != *dir {
            return Err(bad("id is not the directory's name"));
        }
        if profile.version.is_empty() || profile.title.is_empty() || profile.source.is_empty() {
            return Err(bad("version, title and source are required"));
        }
        if profile.shapes.is_empty() && profile.required.is_empty() {
            return Err(bad(
                "shapes or required rows are needed, or nothing checks its feed",
            ));
        }
        let mut named = profile
            .shapes
            .iter()
            .chain(profile.contexts.iter().map(|c| &c.file));
        if let Some(gone) = named.find(|f| profile.file(f).is_empty()) {
            return Err(bad(&format!("{gone} is empty or missing")));
        }
        if profile
            .contexts
            .iter()
            .flat_map(|c| c.prefixes.iter().chain(&c.suffixes))
            .any(|part| part.is_empty())
        {
            return Err(bad("an empty context prefix would take every @context"));
        }
        let mut iris = vec![&profile.uri];
        for row in &profile.required {
            iris.extend([&row.target, &row.path, &row.source]);
        }
        for property in &profile.properties {
            iris.push(&property.predicate);
            iris.extend(&property.class);
            iris.extend(property.values.keys());
        }
        if let Some(wrong) = iris.into_iter().find(|iri| !is_iri(iri)) {
            return Err(bad(&format!("'{wrong}' is not an IRI")));
        }
        let own: BTreeSet<String> = [&profile.id, &profile.uri]
            .into_iter()
            .chain(&profile.aliases)
            .map(|name| name.to_lowercase())
            .collect();
        for name in own {
            if !names.insert(name.clone()) {
                return Err(bad(&format!("'{name}' names another profile already")));
            }
        }
        read.push(profile);
    }

    let mut done: Vec<Profile> = Vec::new();
    while !read.is_empty() {
        let ready = read
            .iter()
            .position(|p| p.bases.iter().all(|b| done.iter().any(|d| d.id == *b)));
        let Some(at) = ready else {
            let left: Vec<&str> = read.iter().map(|p| p.id.as_str()).collect();
            return Err(format!(
                "profiles {left:?}: an unknown base or a cycle of bases"
            ));
        };
        let mut profile = read.remove(at);
        for base in profile.bases.clone() {
            if let Some(base) = done.iter().find(|d| d.id == base) {
                profile.inherit(base);
            }
        }
        if profile.serialisations.is_empty() {
            return Err(format!("profile {}: no serialisation", profile.id));
        }
        if profile.render.terms.is_some() != profile.render.personal_data.is_some() {
            return Err(format!(
                "profile {}: terms and personalData go together",
                profile.id
            ));
        }
        done.push(profile);
    }
    if !done.iter().any(|p| p.id == BASE_PROFILE) {
        return Err(format!("no {BASE_PROFILE}"));
    }
    done.sort_by(|a, b| (a.id != BASE_PROFILE, &a.id).cmp(&(b.id != BASE_PROFILE, &b.id)));
    Ok(done)
}

static INSTALLED: LazyLock<std::result::Result<Vec<Profile>, String>> =
    LazyLock::new(|| load(EMBEDDED));
static NONE: LazyLock<Profile> = LazyLock::new(Profile::default);

/// Refuses to start on a manifest that does not load; without it every profiled feed is closed.
pub fn check() -> Result<()> {
    INSTALLED
        .as_ref()
        .map(|_| ())
        .map_err(|e| DataroomError::Rdf(e.clone()))
}

/// A handle on one installed profile.
#[derive(Clone, Copy)]
pub struct DcatProfile(&'static Profile);

impl std::ops::Deref for DcatProfile {
    type Target = Profile;

    fn deref(&self) -> &Profile {
        self.0
    }
}

impl std::fmt::Debug for DcatProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("DcatProfile").field(&self.0.id).finish()
    }
}

impl PartialEq for DcatProfile {
    fn eq(&self, other: &Self) -> bool {
        self.0.id == other.0.id
    }
}

impl Eq for DcatProfile {}

impl DcatProfile {
    /// Every installed profile, EU DCAT-AP first.
    pub fn installed() -> impl Iterator<Item = Self> {
        INSTALLED.as_deref().unwrap_or_default().iter().map(Self)
    }

    /// EU DCAT-AP, which every catalogue serves.
    pub fn eu() -> Self {
        Self::installed().next().unwrap_or_else(|| Self(&NONE))
    }

    pub fn manifest(self) -> &'static Profile {
        self.0
    }

    pub fn is_base(self) -> bool {
        self.0.id == BASE_PROFILE
    }

    pub fn id(self) -> &'static str {
        &self.0.id
    }

    /// The profile of this id, exactly.
    pub fn by_id(id: &str) -> Option<Self> {
        Self::installed().find(|p| p.0.id == id)
    }

    /// The profile a reader names by id, URI or alias, compared whole.
    pub fn from_token(token: &str) -> Option<Self> {
        let token = token.trim().trim_matches(['<', '>', '"']).to_lowercase();
        Self::installed().find(|p| {
            let profile = p.0;
            profile.id.to_lowercase() == token
                || profile.uri.to_lowercase() == token
                || profile.aliases.iter().any(|a| a.to_lowercase() == token)
        })
    }

    pub fn profile_uri(self) -> &'static str {
        &self.0.uri
    }

    pub(crate) fn personal_data_codes(self) -> Option<&'static PersonalData> {
        self.0.render.personal_data.as_ref()
    }

    pub(crate) fn terms(self) -> Option<&'static Terms> {
        self.0.render.terms.as_ref()
    }

    /// Whether the feed needs the catalogue's declared publisher IRI.
    pub fn requires_publisher(self) -> bool {
        self.0
            .catalogue
            .as_ref()
            .is_some_and(|rules| rules.publisher)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EU: &str = r#"{"id":"dcat-ap-3.0.1","version":"1","uri":"urn:x:eu","source":"s",
        "title":{"en":"EU"},"serialisations":["turtle"],"shapes":["a.ttl"],
        "render":{"identifier":true,"licence":true},
        "properties":[{"extra":"a","subject":"dataset","predicate":"urn:x:a","kind":"literal"}]}"#;
    const FILES: &[(&str, &str)] = &[("a.ttl", "# shapes")];

    #[test]
    fn every_installed_manifest_loads() {
        check().expect("profiles");
        assert!(DcatProfile::eu().is_base());
        for profile in DcatProfile::installed() {
            assert_eq!(DcatProfile::from_token(profile.id()), Some(profile));
            assert_eq!(
                DcatProfile::from_token(profile.profile_uri()),
                Some(profile)
            );
            let uppercase = profile.id().to_uppercase();
            assert_eq!(DcatProfile::from_token(&uppercase), Some(profile));
            let longer = format!("{}-x", profile.id());
            assert_eq!(
                DcatProfile::from_token(&longer),
                None,
                "matched whole, not by substring"
            );
        }
    }

    #[test]
    fn a_profile_takes_from_its_bases_and_may_tighten() {
        let child = r#"{"id":"child","version":"1","uri":"urn:x:child","source":"s",
            "title":{"en":"C"},"bases":["dcat-ap-3.0.1"],
            "required":[{"target":"urn:x:T","path":"urn:x:p","source":"urn:x:s"}],
            "render":{"identifier":false},
            "properties":[{"extra":"b","subject":"distribution","predicate":"urn:x:b","kind":"iri"}]}"#;
        let done = load(&[("child", child, &[]), ("dcat-ap-3.0.1", EU, FILES)]).expect("loads");
        let child = &done[1];
        assert_eq!(child.render.identifier, Some(false), "its own");
        assert_eq!(child.render.licence, Some(true), "its base's");
        assert_eq!(child.properties.len(), 2, "properties add up");
        assert_eq!(child.serialisations, [RdfFormat::Turtle]);
        assert!(child.shapes_turtle().contains("<urn:x:p>"));
    }

    #[test]
    fn a_bad_manifest_is_refused_naming_its_directory() {
        let child = |body: &str| {
            format!(
                r#"{{"id":"c","version":"1","uri":"urn:x:c","source":"s","title":{{"en":"C"}},{body}}}"#
            )
        };
        let row = r#""required":[{"target":"urn:x:T","path":"urn:x:p","source":"urn:x:s"}]"#;
        for bad in [
            child(&format!(r#""bases":["nobody"],{row}"#)),
            child(&format!(r#""bases":["c"],{row}"#)),
            child(&format!(
                r#""bases":["dcat-ap-3.0.1"],"aliases":["urn:x:eu"],{row}"#
            )),
            child(&format!(r#""bases":["dcat-ap-3.0.1"],"renderer":1,{row}"#)),
            child(r#""bases":["dcat-ap-3.0.1"]"#),
            child(r#""bases":["dcat-ap-3.0.1"],"shapes":["none.ttl"]"#),
            child(
                r#""bases":["dcat-ap-3.0.1"],"required":[{"target":"a b:c","path":"urn:x:p","source":"urn:x:s"}]"#,
            ),
        ] {
            let text: &'static str = Box::leak(bad.into_boxed_str());
            let err = load(&[("dcat-ap-3.0.1", EU, FILES), ("c", text, &[])]).expect_err(text);
            assert!(err.contains('c'), "{err}");
        }
    }
}
