#![forbid(unsafe_code)]
//! Organization domain DNS TXT verification (`OrganizationDomain/RequestVerification` and
//! `OrganizationDomain/Verify`, W3, K13).
//!
//! - `RequestVerification` issues a TXT record `_kaisatsu-challenge.<domain>` with value
//!   `kaisatsu-verify=<random>` and stores only `sha256_hex("kaisatsu-verify=<random>")` and a
//!   24-hour expiry on the `OrganizationDomain` record.
//! - `Verify` resolves TXT records for `_kaisatsu-challenge.<domain>` through [`DnsResolver`],
//!   refuses cluster-internal suffixes (`.cluster.local`, `.svc`, `.local`, `.internal`,
//!   `localhost`), compares the hashed TXT values in constant time (`subtle`), and sets
//!   `verified = true`.

use chrono::{Duration, Utc};
use futures_core::future::BoxFuture;
use kaisatsu_core::{KaisatsuError, Organization, Result, Store};
use kaisatsu_crypto::{constant_time_str_eq, random_handle_256, sha256_hex};
use parking_lot::RwLock;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

pub const CHALLENGE_PREFIX: &str = "_kaisatsu-challenge.";
pub const TXT_VALUE_PREFIX: &str = "kaisatsu-verify=";
pub const CHALLENGE_TTL_HOURS: i64 = 24;

const FORBIDDEN_DNS_SUFFIXES: &[&str] = &[
    ".cluster.local",
    ".svc",
    ".local",
    ".internal",
    ".localhost",
    "localhost",
];

/// Validates that `domain` is a public DNS name and never under a cluster-internal suffix (K13).
pub fn validate_verifiable_domain(domain: &str) -> Result<String> {
    let clean = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    if clean.is_empty()
        || !clean.contains('.')
        || clean
            .bytes()
            .any(|b| !(b.is_ascii_alphanumeric() || b == b'.' || b == b'-'))
    {
        return Err(KaisatsuError::BadRequest("invalid domain name".into()));
    }
    for suffix in FORBIDDEN_DNS_SUFFIXES {
        if clean == *suffix || clean.ends_with(suffix) {
            return Err(KaisatsuError::OutboundRefused(
                "DNS resolution of cluster-internal domain suffix is refused".into(),
            ));
        }
    }
    Ok(clean)
}

/// Trait for resolving DNS TXT records so tests can inject canned TXT records and production
/// resolves via `hickory-resolver` behind the outbound domain policy (K13).
pub trait DnsResolver: Send + Sync {
    fn lookup_txt<'a>(&'a self, fqdn: &'a str) -> BoxFuture<'a, Result<Vec<String>>>;
}

/// Production DNS TXT resolver backed by `hickory-resolver` 0.26 with a 5-second timeout and
/// cluster-suffix refusal, plus an optional test mock map.
#[derive(Clone, Default)]
pub struct HickoryDnsResolver {
    mocks: Arc<RwLock<HashMap<String, Vec<String>>>>,
}

impl HickoryDnsResolver {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_mock_txt(&self, fqdn: impl Into<String>, records: Vec<String>) {
        self.mocks.write().insert(
            fqdn.into().trim_end_matches('.').to_ascii_lowercase(),
            records,
        );
    }
}

impl DnsResolver for HickoryDnsResolver {
    fn lookup_txt<'a>(&'a self, fqdn: &'a str) -> BoxFuture<'a, Result<Vec<String>>> {
        Box::pin(async move {
            let clean = fqdn.trim().trim_end_matches('.').to_ascii_lowercase();
            let base = clean.strip_prefix(CHALLENGE_PREFIX).unwrap_or(&clean);
            let _ = validate_verifiable_domain(base)?;

            if let Some(mocked) = self.mocks.read().get(&clean).cloned() {
                return Ok(mocked);
            }

            let resolver = hickory_resolver::Resolver::builder_tokio()
                .map_err(|e| KaisatsuError::OutboundRefused(format!("DNS resolver init: {e}")))?
                .build()
                .map_err(|e| KaisatsuError::OutboundRefused(format!("DNS resolver build: {e}")))?;
            let response = resolver
                .txt_lookup(format!("{clean}."))
                .await
                .map_err(|_| {
                    KaisatsuError::BadRequest("DNS TXT lookup returned no records".into())
                })?;
            let mut out = Vec::new();
            for record in response.answers() {
                if let hickory_resolver::proto::rr::RData::TXT(txt) = &record.data {
                    out.push(txt.to_string());
                }
            }
            Ok(out)
        })
    }
}

/// Finds the organization owning `domain_key` (by domain `id` or domain `name`) within the
/// caller's allowed organizations. Returns 404 when outside scope or missing.
pub async fn find_visible_domain(
    store: &Arc<dyn Store>,
    realm_id: &str,
    allowed_orgs: Option<&[String]>,
    domain_key: &str,
) -> Result<(Organization, usize)> {
    let orgs = store.list_organizations(realm_id, allowed_orgs).await?;
    for org in orgs {
        if let Some(idx) = org
            .domains
            .iter()
            .position(|d| d.id == domain_key || d.name.eq_ignore_ascii_case(domain_key))
        {
            return Ok((org, idx));
        }
    }
    Err(KaisatsuError::NotFound("domain not found".into()))
}

/// Issues a fresh DNS TXT challenge for `domain_key`, stores its SHA-256 hash and expiry on the
/// domain, and returns `{ txtRecord, txtValue, expiresAt }` (K13).
pub async fn request_domain_verification(
    store: &Arc<dyn Store>,
    realm_id: &str,
    allowed_orgs: Option<&[String]>,
    domain_key: &str,
) -> Result<Value> {
    let (mut org, idx) = find_visible_domain(store, realm_id, allowed_orgs, domain_key).await?;
    let clean_domain = validate_verifiable_domain(&org.domains[idx].name)?;
    let txt_value = format!("{TXT_VALUE_PREFIX}{}", random_handle_256());
    let expires_at = Utc::now() + Duration::hours(CHALLENGE_TTL_HOURS);

    org.domains[idx].challenge_hash = Some(sha256_hex(&txt_value));
    org.domains[idx].challenge_expires_at = Some(expires_at);
    org.domains[idx].verification_token = None;
    store.upsert_organization(org).await?;

    Ok(json!({
        "domain": clean_domain,
        "txtRecord": format!("{CHALLENGE_PREFIX}{clean_domain}"),
        "txtValue": txt_value,
        "expiresAt": expires_at.to_rfc3339(),
    }))
}

/// Resolves `_kaisatsu-challenge.<domain>` via `resolver`, compares each TXT value's SHA-256
/// against `challenge_hash` in constant time, and marks `verified = true` on match (K13).
pub async fn verify_organization_domain(
    store: &Arc<dyn Store>,
    resolver: &dyn DnsResolver,
    realm_id: &str,
    allowed_orgs: Option<&[String]>,
    domain_key: &str,
) -> Result<Value> {
    let (mut org, idx) = find_visible_domain(store, realm_id, allowed_orgs, domain_key).await?;
    let dom = &org.domains[idx];
    let clean_domain = validate_verifiable_domain(&dom.name)?;

    let expected_hash = dom
        .challenge_hash
        .as_deref()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| {
            KaisatsuError::BadRequest(
                "no active verification challenge for this domain; call RequestVerification first"
                    .into(),
            )
        })?
        .to_owned();
    let expires_at = dom
        .challenge_expires_at
        .ok_or_else(|| KaisatsuError::BadRequest("verification challenge has no expiry".into()))?;
    if expires_at <= Utc::now() {
        return Err(KaisatsuError::BadRequest(
            "domain verification challenge has expired".into(),
        ));
    }

    let fqdn = format!("{CHALLENGE_PREFIX}{clean_domain}");
    let records = resolver.lookup_txt(&fqdn).await?;
    if records.is_empty() {
        return Err(KaisatsuError::BadRequest(
            "no DNS TXT record found at challenge name".into(),
        ));
    }

    let mut matched = false;
    for rec in &records {
        let candidate = rec.trim().trim_matches('"');
        let cand_hash = sha256_hex(candidate);
        if constant_time_str_eq(&cand_hash, &expected_hash) {
            matched = true;
        }
    }
    if !matched {
        return Err(KaisatsuError::BadRequest(
            "DNS TXT record value does not match challenge".into(),
        ));
    }

    org.domains[idx].verified = true;
    org.domains[idx].challenge_hash = None;
    org.domains[idx].challenge_expires_at = None;
    let updated_dom = org.domains[idx].clone();
    store.upsert_organization(org).await?;

    Ok(kaisatsu_core::odata_defs::org_domain_to_row(&updated_dom))
}
