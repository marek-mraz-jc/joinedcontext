//! Shared name and identifier validators across joinedcontext platform kinds.

use crate::error::{Error, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

static DNS1123_LABEL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9]([-a-z0-9]*[a-z0-9])?$").expect("valid regex"));
static PROJECT_SLUG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9-]{0,62}$").expect("valid regex"));
static SPACE_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9-]{0,62}$").expect("valid regex"));
static DNS_LABEL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$").expect("valid regex"));
static LOCAL_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9._~-]{1,128}$").expect("valid regex"));
static ENTITY_TYPE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z][A-Za-z0-9]{1,63}$").expect("valid regex"));
static LOCALE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z]{2}$").expect("valid regex"));

/// Validates a DNS-1123 label for metadata names (`^[a-z0-9]([-a-z0-9]*[a-z0-9])?$`, 1..=63 characters) (MF-02).
pub fn validate_dns1123_label(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 63 || !DNS1123_LABEL_RE.is_match(name) {
        return Err(Error::Name {
            field: "metadata.name",
            value: name.to_string(),
            reason: "must match DNS-1123 label regex ^[a-z0-9]([-a-z0-9]*[a-z0-9])?$ and be 1 to 63 characters",
        });
    }
    Ok(())
}

/// Every name the platform's routing gives a meaning of its own (T-6974): the first path
/// segments jc places on a shared host (module slugs, the gateway's and the assistant's root
/// paths, jc's and the Portal's own pages and the deployment's extra prefixes), every host label
/// below the deployment's domain (module hosts, the deployment's `idm`, `data`, `status`, `ai`,
/// `mqtt`), the segments docs/API/15-addresses-v1.md fixes (`data`, `dash`, `dsc`, `app`, `c`) and
/// labels held for hosts not served yet (`login`, `embed`, `apps`, ...). An Organization named like one
/// would take that segment or label from the service on a host the Organizations share, so no
/// Organization takes one. The list only grows; bins/jc fails a test when a mount, host label or
/// Portal page root it places is missing here. No live Organization's name is on it: `bbsk`
/// (jc-prod's landing host is named after it) and `ops` (a jc-dev Organization) stay out.
pub const RESERVED_LABELS: &[&str] = &[
    "accessibility",
    "admin",
    "ai",
    "ai-connectors",
    "api",
    "app",
    "apps",
    "assets",
    "assistant",
    "autodiscover",
    "broker",
    "c",
    "callback",
    "catalog",
    "catalogue",
    "closed",
    "connect",
    "connectors",
    "cs",
    "d",
    "dash",
    "dashboards",
    "data",
    "docs",
    "dsc",
    "embed",
    "endpoints",
    "f",
    "git",
    "glossary",
    "grafana",
    "healthz",
    "id",
    "idm",
    "login",
    "logout",
    "mail",
    "mcp",
    "mcp-confirmations",
    "metrics",
    "model-tools",
    "mqtt",
    "oauth",
    "opendata",
    "organization",
    "playground",
    "portal",
    "portals",
    "projects",
    "settings",
    "status",
    "streams",
    "tiles",
    "v",
    "viz",
    "www",
];

/// Refuses an Organization name the platform's routing reserves ([`RESERVED_LABELS`]).
pub fn validate_not_reserved(name: &str) -> Result<()> {
    if RESERVED_LABELS.contains(&name) {
        return Err(Error::Name {
            field: "metadata.name",
            value: name.to_string(),
            reason: "is reserved for the platform: an address of a platform service uses this name; choose another Organization name",
        });
    }
    Ok(())
}

/// Validates a target namespace: literal `org` or project slug (`^[a-z0-9][a-z0-9-]{0,62}$`) (MF-02).
pub fn validate_namespace(ns: &str) -> Result<()> {
    if ns.is_empty() || ns.len() > 63 || !PROJECT_SLUG_RE.is_match(ns) {
        return Err(Error::Name {
            field: "metadata.namespace",
            value: ns.to_string(),
            reason: "must be `org` or match ^[a-z0-9][a-z0-9-]{0,62}$ and be 1 to 63 characters",
        });
    }
    Ok(())
}

/// Validates a Context Space name (`^[a-z0-9][a-z0-9-]{0,62}$`) (PF-09).
pub fn validate_space_name(space: &str) -> Result<()> {
    if space.is_empty() || space.len() > 63 || !SPACE_NAME_RE.is_match(space) {
        return Err(Error::Name {
            field: "space",
            value: space.to_string(),
            reason: "must match ^[a-z0-9][a-z0-9-]{0,62}$ and be 1 to 63 characters",
        });
    }
    Ok(())
}

/// Validates an organization's verified domain name (PF-41, PF-42).
pub fn validate_org_domain(domain: &str) -> Result<()> {
    if domain.is_empty()
        || domain.len() > 253
        || domain.starts_with('.')
        || domain.ends_with('.')
        || domain.contains(':')
        || domain.contains('/')
    {
        return Err(Error::Name {
            field: "orgDomain",
            value: domain.to_string(),
            reason: "must be a lowercase DNS name with at least two labels and total length <= 253",
        });
    }

    let labels: Vec<&str> = domain.split('.').collect();
    if labels.len() < 2 {
        return Err(Error::Name {
            field: "orgDomain",
            value: domain.to_string(),
            reason: "must contain at least two labels separated by dots",
        });
    }

    for label in labels {
        if label.is_empty() || label.len() > 63 || !DNS_LABEL_RE.is_match(label) {
            return Err(Error::Name {
                field: "orgDomain",
                value: domain.to_string(),
                reason: "each label must match ^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$ and be 1 to 63 characters",
            });
        }
    }

    Ok(())
}

/// The Organization id of a verified domain: its labels reversed and joined by `_` (`bbsk.sk` ->
/// `sk_bbsk`, `praha.eu` -> `eu_praha`), the owner segment of every entity URN the Organization's
/// spaces hold (ADR-N-068, EP-186). A hostname holds no `_`, so the map is one-to-one.
pub fn org_id(domain: &str) -> String {
    domain.rsplit('.').collect::<Vec<_>>().join("_")
}

/// An Organization id (API/15 section 2, EP-186): the verified domain's labels reversed and
/// joined by `_` (`bbsk.sk` -> `sk_bbsk`), `[a-z][a-z0-9_-]{1,62}` with at least one `_`, each
/// `_`-separated part a DNS label. Every other name of the platform is DNS-1123 and holds no
/// `_`, so an id never equals a project, space, Endpoint, App or resource name, and no such
/// name is ever read as an id.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct OrganizationId(String);

impl OrganizationId {
    /// Parses an id as written (`sk_bbsk`).
    pub fn parse(id: &str) -> Result<Self> {
        let parts_ok = id
            .split('_')
            .all(|part| part.len() <= 63 && DNS_LABEL_RE.is_match(part));
        if !(2..=63).contains(&id.len())
            || !id.starts_with(|c: char| c.is_ascii_lowercase())
            || !id.contains('_')
            || !parts_ok
            || id.starts_with("snap-")
        {
            return Err(Error::Name {
                field: "organizationId",
                value: id.to_string(),
                reason: "must be a verified domain with its labels reversed and joined by `_` (`sk_bbsk` for bbsk.sk): [a-z][a-z0-9_-]{1,62} with a `_`, not starting `snap-`",
            });
        }
        Ok(Self(id.to_owned()))
    }

    /// The id of a verified domain: [`validate_org_domain`], then [`org_id`], at most 63 long.
    pub fn from_domain(domain: &str) -> Result<Self> {
        validate_org_domain(domain)?;
        Self::parse(&org_id(domain)).map_err(|_| Error::Name {
            field: "orgDomain",
            value: domain.to_string(),
            reason: "gives an Organization id longer than 63 characters or not starting with a letter: choose a shorter domain",
        })
    }

    /// The id as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for OrganizationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for OrganizationId {
    type Err = Error;
    fn from_str(id: &str) -> Result<Self> {
        Self::parse(id)
    }
}

impl<'de> Deserialize<'de> for OrganizationId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// A broker tenant (PF-130, EP-22): `{organization id}__{space segment}` (`sk_bbsk__mobility`),
/// at most 63 + 2 + 63 = 128 characters. An [`OrganizationId`] never holds `__` and a space
/// segment holds no `_`, so the name splits back at its first `__` into exactly one pair, and
/// two Organizations never share a tenant.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct TenantName(String);

impl TenantName {
    /// The tenant of the space `segment` of Organization `org`.
    pub fn new(org: &OrganizationId, segment: &str) -> Result<Self> {
        validate_space_name(segment)?;
        Ok(Self(format!("{org}__{segment}")))
    }

    /// Parses a tenant as written: splits at the first `__`, then checks each half by its own
    /// grammar, so `sk_a` never matches `sk_ab__x` and no prefix compare is needed.
    pub fn parse(raw: &str) -> Result<Self> {
        let refused = || Error::Name {
            field: "tenant",
            value: raw.to_string(),
            reason:
                "must be `{organization id}__{space}` (`sk_bbsk__mobility`), at most 128 characters",
        };
        let (org, segment) = raw.split_once("__").ok_or_else(refused)?;
        let org = OrganizationId::parse(org).map_err(|_| refused())?;
        Self::new(&org, segment).map_err(|_| refused())
    }

    /// The Organization id half.
    #[must_use]
    pub fn organization(&self) -> &str {
        self.0.split_once("__").map_or("", |(org, _)| org)
    }

    /// The space segment half.
    #[must_use]
    pub fn segment(&self) -> &str {
        self.0.split_once("__").map_or("", |(_, segment)| segment)
    }

    /// The name as text, as the `NGSILD-Tenant` header carries it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TenantName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for TenantName {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// Validates a local identifier within an entity URN (`^[A-Za-z0-9._~-]{1,128}$`) (PF-42).
pub fn validate_local_id(local_id: &str) -> Result<()> {
    if local_id.is_empty() || local_id.len() > 128 || !LOCAL_ID_RE.is_match(local_id) {
        return Err(Error::Name {
            field: "localId",
            value: local_id.to_string(),
            reason: "must match ^[A-Za-z0-9._~-]{1,128}$ and be 1 to 128 characters",
        });
    }
    Ok(())
}

/// Validates an entity type short name in PascalCase (`^[A-Z][A-Za-z0-9]{1,63}$`) (PF-42).
pub fn validate_entity_type(t: &str) -> Result<()> {
    if t.len() < 2 || t.len() > 64 || !ENTITY_TYPE_RE.is_match(t) {
        return Err(Error::Name {
            field: "entityType",
            value: t.to_string(),
            reason:
                "must match PascalCase regex ^[A-Z][A-Za-z0-9]{1,63}$ and be 2 to 64 characters",
        });
    }
    Ok(())
}

/// Validates an ISO 639-1 two-letter lowercase language code (`^[a-z]{2}$`) (PF-25).
pub fn validate_locale(l: &str) -> Result<()> {
    if l.len() != 2 || !LOCALE_RE.is_match(l) {
        return Err(Error::Locale(l.to_string()));
    }
    Ok(())
}

/// The prefixes a minted credential wears, lowercased, with the separator both spellings use.
///
/// Only shapes a token announces itself by: a rule that guessed from length or entropy would
/// refuse real secret names, and a name nobody can use is a worse failure than the one this
/// catches. The list is the tokens a person has in their clipboard while they configure a source.
const CREDENTIAL_PREFIXES: &[&str] = &[
    "ghp_",
    "ghp-",
    "gho_",
    "gho-",
    "ghu_",
    "ghu-",
    "ghs_",
    "ghs-",
    "ghr_",
    "ghr-",
    "github_pat_",
    "github-pat-",
    "glpat_",
    "glpat-",
    "gldt_",
    "gldt-",
    "glrt_",
    "glrt-",
    "xoxb_",
    "xoxb-",
    "xoxp_",
    "xoxp-",
    "xoxa_",
    "xoxa-",
    "xapp_",
    "xapp-",
    "sk_",
    "sk-",
    "pk_live_",
    "pk-live-",
    "sk_live_",
    "sk-live-",
    "rk_live_",
    "rk-live-",
    "dckr_pat_",
    "dckr-pat-",
    "npm_",
    "npm-",
    "hf_",
    "hf-",
    "sbp_",
    "sbp-",
    "ya29.",
];

/// Whether `value` is a key whose prefix is also a word (`Asia`, `Aizawl`), so only its whole
/// shape is taken for one (F-R2PLT3-011): an AWS access key id, `AKIA` or `ASIA` and 16 capitals
/// or digits, and a Google API key, `AIza` and 35 more characters.
fn is_cloud_key(value: &str) -> bool {
    let tail = |prefix: &str, len: usize, ok: fn(&u8) -> bool| {
        value
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.len() == len && rest.bytes().all(|b| ok(&b)))
    };
    let aws = |b: &u8| b.is_ascii_uppercase() || b.is_ascii_digit();
    let google = |b: &u8| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_';
    tail("AKIA", 16, aws) || tail("ASIA", 16, aws) || tail("AIza", 35, google)
}

/// Whether a value carries the shape of a minted credential rather than the name of one (MF-24).
///
/// Used where a person types the *name* of a secret: a name is free text, and the mistake this
/// catches is pasting the credential itself into it, which would commit a live token to Git. A
/// token whose prefix is also a valid DNS-1123 label (`glpat-…`, `xoxb-…`, `sk-…`) passes every
/// other rule the platform has, which is why this one exists.
pub fn looks_like_a_credential(value: &str) -> bool {
    let lowered = value.to_ascii_lowercase();
    if CREDENTIAL_PREFIXES
        .iter()
        .any(|prefix| lowered.starts_with(prefix))
    {
        return true;
    }
    if is_cloud_key(value) {
        return true;
    }
    // A PEM block or a JWT: three dot-separated base64url segments starting with a JSON header.
    lowered.starts_with("-----begin") || (value.starts_with("eyJ") && value.split('.').count() == 3)
}

/// A validator's name error, reported against the manifest field the caller checked rather than
/// the one the shared validator named.
pub(crate) fn rename(err: Error, field: &'static str) -> Error {
    match err {
        Error::Name { reason, value, .. } => Error::Name {
            field,
            value,
            reason,
        },
        other => other,
    }
}

/// A path beside a manifest: relative, not empty (a leading `./` is allowed and does not count),
/// with no `..` segment, so it cannot name a file outside the repository.
pub(crate) fn validate_relative_path(field: &'static str, path: &str) -> Result<()> {
    let trimmed = path.strip_prefix("./").unwrap_or(path);
    if trimmed.is_empty() || trimmed.starts_with('/') || trimmed.split('/').any(|seg| seg == "..") {
        return Err(Error::Name {
            field,
            value: path.to_string(),
            reason: "path must be relative and must not contain a `..` segment",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_labels_are_sorted_unique_labels() {
        assert!(RESERVED_LABELS.windows(2).all(|w| w[0] < w[1]));
        for name in RESERVED_LABELS {
            validate_dns1123_label(name).expect(name);
        }
    }

    /// T-6974: every door that parses an Organization (the Portal's judge, import, the
    /// reconciler's load, `jcctl validate`) reaches this check through `validate_spec`.
    #[test]
    fn an_organization_manifest_with_a_reserved_name_is_refused() {
        let golden = include_str!("../tests/golden/004-Organization-03-domain-model.yaml");
        let named =
            |name: &str| golden.replacen("name: banskabystrica", &format!("name: {name}"), 1);
        crate::registry::validate_yaml("Organization", golden)
            .expect("registered")
            .expect("the golden passes");
        for name in ["api", "opendata", "oauth", "login"] {
            let err = crate::registry::validate_yaml("Organization", &named(name))
                .expect("registered")
                .expect_err(name);
            assert!(
                err.to_string().contains("reserved for the platform"),
                "{name}: {err}"
            );
        }
    }

    #[test]
    fn a_reserved_name_is_refused_and_an_ordinary_one_passes() {
        for name in ["api", "opendata", "oauth", "login"] {
            let err = validate_not_reserved(name).expect_err(name);
            assert!(
                err.to_string().contains("reserved for the platform"),
                "{err}"
            );
        }
        for name in [
            "bbsk",
            "banskabystrica",
            "hel",
            "bratislava",
            "praha",
            "barcelona",
            "zilina",
            "slovensko",
            "ctl-a",
            "ctl-b",
            "demo-mcp",
            "ops",
            "helsinki",
        ] {
            validate_not_reserved(name).expect(name);
        }
    }

    /// T-6769 (ADR-N-068): the owner segment is the text before the first `:` of the id part,
    /// and a URN with nothing after it, or without a `:`, names no owner.
    #[test]
    fn the_owner_segment_is_the_first_of_the_id() {
        for (urn, owner) in [
            (
                "urn:ngsi-ld:KeyPerformanceIndicator:sk_bbsk:bbsk-otvorene-data:k1",
                Some("sk_bbsk"),
            ),
            ("urn:ngsi-ld:Device:sk_bratislava:1", Some("sk_bratislava")),
            ("urn:ngsi-ld:Device:bbsk.sk:space:1", Some("bbsk.sk")),
            ("urn:ngsi-ld:WeatherObserved:Helsinki-001", None),
        ] {
            let parsed: crate::Urn = urn.parse().expect("a URN");
            assert_eq!(parsed.owner(), owner, "{urn}");
        }
    }

    /// T-6769: the four Organizations of the owner's order, and a domain of three labels.
    #[test]
    fn an_org_id_is_the_domain_reversed() {
        for (domain, id) in [
            ("bbsk.sk", "sk_bbsk"),
            ("bratislava.sk", "sk_bratislava"),
            ("praha.eu", "eu_praha"),
            ("hel.fi", "fi_hel"),
            ("banskabystrica.sk", "sk_banskabystrica"),
            ("city.example.com", "com_example_city"),
        ] {
            assert_eq!(org_id(domain), id);
        }
    }

    /// T-7007 (EP-186): an id is the reversed verified domain, never a DNS-1123 name.
    #[test]
    fn an_organization_id_is_parsed_and_minted_from_its_domain() {
        for id in [
            "sk_bbsk",
            "sk_marek-mraz",
            "com_joinedcontext_region",
            "xn--p1ai_example",
        ] {
            assert_eq!(OrganizationId::parse(id).expect(id).as_str(), id);
        }
        for bad in [
            "bbsk",
            "marek-mraz",
            "com-marek-mraz",
            "Sk_bbsk",
            "1sk_bbsk",
            "sk__bbsk",
            "sk_",
            "_sk",
            "sk_-bbsk",
            "sk_bbsk-",
            "sk.bbsk",
            "sk:bbsk",
            "",
            &"a_".repeat(32),
        ] {
            assert!(OrganizationId::parse(bad).is_err(), "{bad}");
        }
        assert_eq!(
            OrganizationId::from_domain("marek-mraz.sk")
                .unwrap()
                .to_string(),
            "sk_marek-mraz"
        );
        let long = format!("{}.{}.sk", "a".repeat(40), "b".repeat(40));
        match OrganizationId::from_domain(&long) {
            Err(Error::Name { field, .. }) => assert_eq!(field, "orgDomain"),
            other => panic!("{other:?}"),
        }
        assert!(OrganizationId::from_domain("bbsk").is_err());
        let parsed: OrganizationId = serde_json::from_str("\"fi_hel\"").unwrap();
        assert_eq!(serde_json::to_string(&parsed).unwrap(), "\"fi_hel\"");
        assert!(serde_json::from_str::<OrganizationId>("\"hel\"").is_err());
    }

    /// Every string over `{a, 0, -, _}` up to `max` characters.
    fn strings(max: usize) -> Vec<String> {
        let mut all = vec![String::new()];
        let mut last = vec![String::new()];
        for _ in 0..max {
            last = last
                .iter()
                .flat_map(|s| ['a', '0', '-', '_'].map(|c| format!("{s}{c}")))
                .collect();
            all.extend(last.iter().cloned());
        }
        all
    }

    /// PF-130: every Organization id and space segment up to 6 characters (plus the 63-character
    /// boundary) mints a tenant that splits back into the same pair and that the broker's own
    /// grammar accepts; every string up to 8 parses only as the pair that minted it.
    #[test]
    fn a_tenant_name_splits_back_into_its_organization_and_segment() {
        let long_org = format!("{}_b", "a".repeat(61));
        let long_segment = "s".repeat(63);
        let mut ids: Vec<OrganizationId> = strings(6)
            .iter()
            .filter_map(|s| OrganizationId::parse(s).ok())
            .collect();
        ids.push(OrganizationId::parse(&long_org).expect("63-character id"));
        let mut segments: Vec<String> = strings(6)
            .into_iter()
            .filter(|s| validate_space_name(s).is_ok())
            .collect();
        segments.push(long_segment.clone());
        assert!(ids.len() > 100 && segments.len() > 100);
        for org in &ids {
            for segment in &segments {
                let tenant = TenantName::new(org, segment).expect("valid pair");
                let parsed = TenantName::parse(tenant.as_str()).expect(tenant.as_str());
                assert_eq!(
                    (parsed.organization(), parsed.segment()),
                    (org.as_str(), segment.as_str())
                );
                assert!(
                    antares_model::id::TenantId::new(tenant.as_str()).is_ok(),
                    "{tenant}"
                );
            }
        }
        let widest =
            TenantName::new(&OrganizationId::parse(&long_org).unwrap(), &long_segment).unwrap();
        assert_eq!(widest.as_str().len(), 128);
        for raw in strings(8) {
            if let Ok(tenant) = TenantName::parse(&raw) {
                assert_eq!(tenant.as_str(), raw);
                let org = OrganizationId::parse(tenant.organization()).expect(&raw);
                assert_eq!(
                    TenantName::new(&org, tenant.segment()).unwrap().as_str(),
                    raw
                );
            }
        }
    }

    /// PF-130: no `__`, two `__`, a `_` in the segment, an empty half, a bad id or segment, and
    /// a name over 128 are refused; an id starting `snap-` (the broker's reserved prefix) is
    /// refused while look-alikes pass; serde reads through the same parse.
    #[test]
    fn a_tenant_name_refuses_every_other_shape() {
        let over = format!("{}_b__{}", "a".repeat(61), "s".repeat(64));
        for bad in [
            "sk_bbsk",
            "sk_bbsk-mobility",
            "sk_a__b__c",
            "sk_a__b_c",
            "__mobility",
            "sk_a__",
            "bbsk__mobility",
            "Sk_a__x",
            "sk_a__-x",
            "sk_a__X",
            "snap-a_b__x",
            &over,
            "default",
            "",
        ] {
            assert!(TenantName::parse(bad).is_err(), "{bad}");
        }
        assert!(OrganizationId::parse("snap-a_b").is_err());
        for good in ["snapx_b", "sk_snap-a"] {
            assert!(OrganizationId::parse(good).is_ok(), "{good}");
        }
        let tenant = TenantName::parse("sk_bbsk__helsinki-mobility").unwrap();
        assert_eq!(
            (tenant.organization(), tenant.segment()),
            ("sk_bbsk", "helsinki-mobility")
        );
        assert!(serde_json::from_str::<TenantName>("\"sk_bbsk-mobility\"").is_err());
        assert_eq!(
            serde_json::to_string(&tenant).unwrap(),
            "\"sk_bbsk__helsinki-mobility\""
        );
    }

    /// T-7007: no other name can take the shape of an Organization id. Every validator but the
    /// entity's local id refuses `_`, and every golden manifest renamed to an id (its name or
    /// its namespace) is refused at the one parse every door calls, while a DNS-1123 look-alike
    /// passes.
    #[test]
    fn no_other_name_can_be_an_organization_id() {
        for id in ["sk_bbsk", "com_marek-mraz"] {
            assert!(validate_dns1123_label(id).is_err(), "{id}");
            assert!(validate_namespace(id).is_err(), "{id}");
            assert!(validate_space_name(id).is_err(), "{id}");
            assert!(validate_org_domain(id).is_err(), "{id}");
            assert!(validate_entity_type(id).is_err(), "{id}");
            assert!(validate_locale(id).is_err(), "{id}");
        }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
        let mut checked = std::collections::BTreeSet::new();
        for file in std::fs::read_dir(dir).unwrap() {
            let raw = std::fs::read_to_string(file.unwrap().path()).unwrap();
            let doc: serde_json::Value = serde_norway::from_str(&raw).unwrap();
            let kind = doc["kind"].as_str().unwrap().to_owned();
            let valid = |doc: &serde_json::Value| {
                let yaml = serde_norway::to_string(doc).unwrap();
                crate::registry::validate_yaml(&kind, &yaml).expect("a known kind")
            };
            if valid(&doc).is_err() {
                continue;
            }
            for field in ["name", "namespace"] {
                if doc["metadata"].get(field).is_none() {
                    continue;
                }
                for id in ["sk_bbsk", "com_marek-mraz"] {
                    let mut renamed = doc.clone();
                    renamed["metadata"][field] = id.into();
                    assert!(valid(&renamed).is_err(), "{kind} {field} {id}");
                }
            }
            if matches!(
                kind.as_str(),
                "Project" | "ContextSpace" | "Endpoint" | "App"
            ) {
                for name in ["marek-mraz", "com-marek-mraz"] {
                    let mut renamed = doc.clone();
                    renamed["metadata"]["name"] = name.into();
                    valid(&renamed).unwrap_or_else(|e| panic!("{kind} {name}: {e}"));
                    assert!(OrganizationId::parse(name).is_err());
                }
            }
            checked.insert(kind);
        }
        for kind in ["Organization", "Project", "ContextSpace", "Endpoint", "App"] {
            assert!(checked.contains(kind), "no golden {kind} validated");
        }
    }

    /// T-1479: one rule for a path beside a manifest, reported against the caller's field.
    #[test]
    fn a_path_beside_a_manifest_stays_inside_the_repository() {
        for path in ["../x", "a/../../b", "/etc/passwd", "", "./"] {
            match validate_relative_path("spec.tests.input", path) {
                Err(Error::Name { field, value, .. }) => {
                    assert_eq!((field, value.as_str()), ("spec.tests.input", path));
                }
                other => panic!("`{path}` passed: {other:?}"),
            }
        }
        for path in ["tests/in.json", "./tests/in.json", "a/b..c/d"] {
            assert!(validate_relative_path("linkml", path).is_ok(), "{path}");
        }
        let refused = validate_dns1123_label("Not A Label").expect_err("refused");
        let renamed = rename(refused, "spec.target");
        assert!(
            matches!(
                renamed,
                Error::Name {
                    field: "spec.target",
                    ..
                }
            ),
            "{renamed:?}"
        );
    }
}
