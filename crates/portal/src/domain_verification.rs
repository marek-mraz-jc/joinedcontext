//! Whether an Organization owns the domain it declares (PF-41, T-2377, Architecture/03 §3).
//!
//! `spec.domain` is the second segment of every entity id the Organization mints (PF-42) and the
//! `did:web` its policies are signed as, so a declaration nobody checked lets an installation
//! speak as somebody else's domain. Each Organization gets one challenge, minted once and kept
//! in the database, and proves the domain by either of two things it publishes:
//!
//! - the TXT record `_joinedcontext.{domain}` holding `jc-verify={challenge}`, or
//! - `https://{domain}/.well-known/did.json` naming this instance's host among its services.
//!
//! The outcome is `status.domainVerification` on the Organization. A `verified` domain is
//! checked again after 30 days; a `pending` or `failed` one every ten minutes, so a record
//! published now is seen soon without the reconciler asking DNS on every pass. This module
//! records and reports; refusing writes (`enforce`) is the gateway's, and the owner's switch.

use std::time::Duration;

use argon2::password_hash::rand_core::{OsRng, RngCore};
use base64::Engine as _;
use jc_time::TimestampTz;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

/// How long a verified domain stays verified before it is checked again (PF-41).
pub const RECHECK_VERIFIED: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// How long a domain that is not verified waits before the next look.
pub const RECHECK_UNVERIFIED: Duration = Duration::from_secs(10 * 60);
/// How long one lookup, the TXT query or the document fetch, may take.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);
/// The most of a `did.json` read: a DID document is a few kilobytes.
const DID_DOCUMENT_LIMIT: usize = 64 * 1024;

/// Where an Organization's domain stands (PF-41).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum State {
    /// Not looked at yet, or the domain changed since.
    Pending,
    /// The domain published this instance's challenge or names its host.
    Verified,
    /// Looked, and neither proof was there.
    Failed,
}

impl State {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Verified => "verified",
            Self::Failed => "failed",
        }
    }

    fn parse(text: &str) -> Self {
        match text {
            "verified" => Self::Verified,
            "failed" => Self::Failed,
            _ => Self::Pending,
        }
    }
}

/// Which of the two proofs verified the domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum Method {
    #[serde(rename = "dns-txt")]
    DnsTxt,
    #[serde(rename = "did-web")]
    DidWeb,
}

impl Method {
    fn as_str(self) -> &'static str {
        match self {
            Self::DnsTxt => "dns-txt",
            Self::DidWeb => "did-web",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "dns-txt" => Some(Self::DnsTxt),
            "did-web" => Some(Self::DidWeb),
            _ => None,
        }
    }
}

/// `status.domainVerification` of an Organization (PF-41, Architecture/03 §3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DomainVerification {
    pub state: State,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<Method>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub checked_at: Option<TimestampTz>,
    /// Why a `failed` domain failed, in words a person acts on; never a resolver's answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// What the TXT record carries after `jc-verify=`. Not a secret: it is published in DNS.
    pub challenge: String,
    /// The record to publish, spelled out: `_joinedcontext.{domain} TXT "jc-verify={challenge}"`.
    pub record: String,
}

/// 32 random bytes, base64url without padding: not guessable, so a domain that carries it was
/// set up for this Organization of this instance.
pub fn mint_challenge() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The name the TXT record lives at.
pub fn txt_name(domain: &str) -> String {
    format!("_joinedcontext.{domain}")
}

/// The whole record, as the Portal shows it to the person who publishes it.
pub fn record(domain: &str, challenge: &str) -> String {
    format!("{} TXT \"jc-verify={challenge}\"", txt_name(domain))
}

/// Whether one of the TXT strings is exactly this challenge's value.
pub fn txt_proves(records: &[String], challenge: &str) -> bool {
    let wanted = format!("jc-verify={challenge}");
    records.iter().any(|text| text.trim() == wanted)
}

/// Whether a DID document names `host` in a service endpoint: a string, a list of them, or the
/// values of a map, each read as a URL whose host is compared, never as text that may contain it.
pub fn did_names_host(document: &Value, host: &str) -> bool {
    fn endpoints<'a>(value: &'a Value, found: &mut Vec<&'a str>) {
        match value {
            Value::String(url) => found.push(url),
            Value::Array(items) => items.iter().for_each(|item| endpoints(item, found)),
            Value::Object(fields) => fields.values().for_each(|field| endpoints(field, found)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    for service in document["service"].as_array().into_iter().flatten() {
        endpoints(&service["serviceEndpoint"], &mut found);
    }
    found.into_iter().any(|endpoint| {
        url::Url::parse(endpoint)
            .ok()
            .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
            .is_some_and(|named| named == host.to_ascii_lowercase())
    })
}

/// Whether a stored verification is due another look at `now`.
pub fn due(verification: &DomainVerification, now: TimestampTz) -> bool {
    let Some(checked) = verification.checked_at else {
        return true;
    };
    let wait = match verification.state {
        State::Verified => RECHECK_VERIFIED,
        State::Pending | State::Failed => RECHECK_UNVERIFIED,
    };
    let elapsed = now - checked;
    !elapsed.is_negative() && elapsed.unsigned_abs() >= wait
}

/// A lookup that did not answer: a timeout, a refusal, a document that is not JSON. An answer
/// without the proof is an `Ok` that does not prove; the resolver's own words stay out of both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unanswered;

/// The two lookups a check makes; the real one asks DNS and the domain's web server.
pub trait Lookup {
    /// The TXT strings at `name`, each record's strings joined.
    fn txt(
        &self,
        name: &str,
    ) -> impl std::future::Future<Output = Result<Vec<String>, Unanswered>> + Send;
    /// `https://{domain}/.well-known/did.json`, parsed.
    fn did_document(
        &self,
        domain: &str,
    ) -> impl std::future::Future<Output = Result<Value, Unanswered>> + Send;
}

/// One check of `domain` for `challenge`, at `now`: the TXT record first, the DID document
/// second, and a failure that says what was missing for each.
pub async fn check(
    lookup: &impl Lookup,
    domain: &str,
    challenge: &str,
    host: &str,
    now: TimestampTz,
) -> DomainVerification {
    let verified = |method| DomainVerification {
        state: State::Verified,
        method: Some(method),
        checked_at: Some(now),
        reason: None,
        challenge: challenge.to_owned(),
        record: record(domain, challenge),
    };
    let txt = lookup.txt(&txt_name(domain)).await;
    if txt
        .as_ref()
        .is_ok_and(|records| txt_proves(records, challenge))
    {
        return verified(Method::DnsTxt);
    }
    let did = lookup.did_document(domain).await;
    if did
        .as_ref()
        .is_ok_and(|document| did_names_host(document, host))
    {
        return verified(Method::DidWeb);
    }
    let txt_said = match txt {
        Err(Unanswered) => format!("the TXT lookup of {} did not answer", txt_name(domain)),
        _ => format!(
            "no TXT record at {} carries this instance's challenge",
            txt_name(domain)
        ),
    };
    let did_said = match did {
        Err(Unanswered) => format!("https://{domain}/.well-known/did.json could not be read"),
        _ => format!("https://{domain}/.well-known/did.json names no service on {host}"),
    };
    DomainVerification {
        state: State::Failed,
        method: None,
        checked_at: Some(now),
        reason: Some(format!("{txt_said}, and {did_said}")),
        challenge: challenge.to_owned(),
        record: record(domain, challenge),
    }
}

/// Where a domain's DID document lives, unless the domain is an address inside the cluster: the
/// verifier runs in it, and a domain is whatever an org administrator writes (T-3877). A name is
/// judged once it resolves, by the client's [`jc_core::outbound::PublicOnly`] resolver.
fn did_url(domain: &str) -> Result<String, Unanswered> {
    let url = format!("https://{domain}/.well-known/did.json");
    match crate::sync::guard::internal_host(&url) {
        Some(_) => Err(Unanswered),
        None => Ok(url),
    }
}

/// The lookups over the network: the pod's own resolver for TXT, and one HTTPS GET that follows
/// no redirect and reaches public addresses only, so the document is the declared domain's and
/// nobody else's.
pub struct NetLookup {
    resolver: hickory_resolver::TokioResolver,
    http: reqwest::Client,
}

impl NetLookup {
    pub fn new() -> Result<Self, String> {
        Self::from_system(hickory_resolver::system_conf::read_system_conf())
    }

    /// [`Self::new`] from the system configuration as read, so a test hands it an unreadable one.
    fn from_system(
        system: Result<
            (
                hickory_resolver::config::ResolverConfig,
                hickory_resolver::config::ResolverOpts,
            ),
            hickory_resolver::net::NetError,
        >,
    ) -> Result<Self, String> {
        let (config, options) =
            system.map_err(|e| format!("the resolver configuration is unreadable: {e}"))?;
        let mut builder = hickory_resolver::TokioResolver::builder_with_config(
            config,
            hickory_resolver::net::runtime::TokioRuntimeProvider::default(),
        );
        *builder.options_mut() = options;
        builder.options_mut().timeout = LOOKUP_TIMEOUT;
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .dns_resolver(std::sync::Arc::new(jc_core::outbound::PublicOnly))
            .timeout(LOOKUP_TIMEOUT)
            .user_agent("joinedcontext-portal domain verification")
            .build()
            .map_err(|e| e.to_string())?;
        let resolver = builder
            .build()
            .map_err(|e| format!("the resolver could not start: {e}"))?;
        Ok(Self { resolver, http })
    }
}

impl Lookup for NetLookup {
    async fn txt(&self, name: &str) -> Result<Vec<String>, Unanswered> {
        match self.resolver.txt_lookup(name).await {
            Ok(answer) => Ok(answer
                .answers()
                .iter()
                .filter_map(|record| match &record.data {
                    hickory_resolver::proto::rr::RData::TXT(txt) => Some(
                        txt.txt_data
                            .iter()
                            .map(|part| String::from_utf8_lossy(part).into_owned())
                            .collect::<String>(),
                    ),
                    _ => None,
                })
                .collect()),
            Err(err) if err.is_no_records_found() || err.is_nx_domain() => Ok(Vec::new()),
            Err(_) => Err(Unanswered),
        }
    }

    async fn did_document(&self, domain: &str) -> Result<Value, Unanswered> {
        let url = did_url(domain)?;
        let mut response = self.http.get(url).send().await.map_err(|_| Unanswered)?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Value::Null);
        }
        if !response.status().is_success() {
            return Err(Unanswered);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Unanswered)? {
            if body.len() + chunk.len() > DID_DOCUMENT_LIMIT {
                return Err(Unanswered);
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map_err(|_| Unanswered)
    }
}

/// The TXT half of [`Lookup`] as an object, so [`crate::state::AppState`] can hold one: the
/// Organization id mint asks it (T-7012), a test hands it canned records.
pub trait TxtRecords: Send + Sync {
    fn txt<'a>(
        &'a self,
        name: &'a str,
    ) -> futures_util::future::BoxFuture<'a, Result<Vec<String>, Unanswered>>;
}

impl TxtRecords for NetLookup {
    fn txt<'a>(
        &'a self,
        name: &'a str,
    ) -> futures_util::future::BoxFuture<'a, Result<Vec<String>, Unanswered>> {
        Box::pin(Lookup::txt(self, name))
    }
}

/// The addresses a registry host resolves to, for its `dns-ok` step (T-7129 §3, T-7138): A and
/// AAAA, a CNAME followed. No record is an empty set, not a failure.
impl crate::reconciler::org_hosts::Addresses for NetLookup {
    fn addresses<'a>(
        &'a self,
        name: &'a str,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<std::collections::BTreeSet<std::net::IpAddr>, Unanswered>,
    > {
        Box::pin(async move {
            match self.resolver.lookup_ip(name).await {
                Ok(answer) => Ok(answer.iter().collect()),
                Err(err) if err.is_no_records_found() || err.is_nx_domain() => {
                    Ok(std::collections::BTreeSet::new())
                }
                Err(_) => Err(Unanswered),
            }
        })
    }
}

/// What the gateway decides a write on: one Organization, its domain and where it stands
/// (`GET /internal/domain-verifications`, Architecture/03 §3). The challenge, the record and
/// the reason stay with the Portal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GateState {
    pub organization: String,
    pub domain: String,
    pub state: State,
}

/// The state of each `(organization, domain)`, from the database every replica shares, so the
/// answer does not depend on which replica the gateway reached. A row for another domain is an
/// Organization whose `spec.domain` changed and has not been checked since; it and an
/// Organization with no row, or a Portal with no database, are `pending`, which is what the
/// gateway refuses under `enforce`.
pub async fn gate_states(
    pool: Option<&sqlx::PgPool>,
    organizations: &[(String, String)],
) -> Result<Vec<GateState>, sqlx::Error> {
    let rows: Vec<(String, String, String)> = match pool {
        Some(pool) => {
            sqlx::query_as("SELECT organization, domain, state FROM domain_verifications")
                .fetch_all(pool)
                .await?
        }
        None => Vec::new(),
    };
    Ok(organizations
        .iter()
        .map(|(organization, domain)| GateState {
            organization: organization.clone(),
            domain: domain.clone(),
            state: rows
                .iter()
                .find(|(name, stored, _)| name == organization && stored == domain)
                .map_or(State::Pending, |(_, _, state)| State::parse(state)),
        })
        .collect())
}

/// The stored verification of each Organization, one row per Organization (PF-41).
pub struct Verifier<L: Lookup> {
    pool: sqlx::PgPool,
    lookup: L,
    /// The host this instance serves, which a `did.json` has to name.
    host: String,
}

impl<L: Lookup> Verifier<L> {
    pub fn new(pool: sqlx::PgPool, lookup: L, host: impl Into<String>) -> Self {
        Self {
            pool,
            lookup,
            host: host.into(),
        }
    }

    /// The verification of `organization` for `domain` at `now`: read, minted on the first
    /// look, reset to `pending` when the domain changed (the challenge stays, it is the
    /// Organization's), checked again when it is due, and written back.
    pub async fn verify(
        &self,
        organization: &str,
        domain: &str,
        now: TimestampTz,
    ) -> Result<DomainVerification, sqlx::Error> {
        // `checked_at` travels as epoch seconds: the status type is jc-time's, sqlx here speaks `time`.
        type Row = (
            String,
            String,
            String,
            Option<String>,
            Option<i64>,
            Option<String>,
        );
        let stored = sqlx::query_as::<_, Row>(
            "SELECT domain, challenge, state, method, \
               extract(epoch FROM checked_at)::bigint, reason \
             FROM domain_verifications WHERE organization = $1",
        )
        .bind(organization)
        .fetch_optional(&self.pool)
        .await?;

        let current = match stored {
            Some((stored_domain, challenge, state, method, checked_at, reason))
                if stored_domain == domain =>
            {
                DomainVerification {
                    state: State::parse(&state),
                    method: method.as_deref().and_then(Method::parse),
                    checked_at: checked_at.and_then(jc_time::TimestampTz::from_second),
                    reason,
                    record: record(domain, &challenge),
                    challenge,
                }
            }
            Some((_, challenge, ..)) => pending(domain, challenge),
            None => pending(domain, mint_challenge()),
        };
        if !due(&current, now) {
            return Ok(current);
        }
        let checked = check(&self.lookup, domain, &current.challenge, &self.host, now).await;
        sqlx::query(
            "INSERT INTO domain_verifications \
               (organization, domain, challenge, state, method, checked_at, reason) \
             VALUES ($1, $2, $3, $4, $5, to_timestamp($6), $7) \
             ON CONFLICT (organization) DO UPDATE SET \
               domain = EXCLUDED.domain, state = EXCLUDED.state, method = EXCLUDED.method, \
               checked_at = EXCLUDED.checked_at, reason = EXCLUDED.reason",
        )
        .bind(organization)
        .bind(domain)
        .bind(&checked.challenge)
        .bind(checked.state.as_str())
        .bind(checked.method.map(Method::as_str))
        .bind(checked.checked_at.map(|at| at.as_second() as f64))
        .bind(&checked.reason)
        .execute(&self.pool)
        .await?;
        Ok(checked)
    }
}

fn pending(domain: &str, challenge: String) -> DomainVerification {
    DomainVerification {
        state: State::Pending,
        method: None,
        checked_at: None,
        reason: None,
        record: record(domain, &challenge),
        challenge,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// T-3877: an Organization's domain is a DNS-shaped name an org administrator writes, and
    /// `10.0.0.1` is one. The verifier runs inside the cluster, so the document is never asked of
    /// an address the cluster keeps to itself; a public name is asked as before.
    #[test]
    fn a_did_document_is_never_fetched_from_an_internal_address() {
        for domain in [
            "10.0.0.1",
            "127.0.0.1",
            "169.254.169.254",
            "100.64.0.10",
            "192.168.1.1",
        ] {
            assert_eq!(did_url(domain), Err(Unanswered), "{domain}");
        }
        assert_eq!(
            did_url("hel.fi").as_deref(),
            Ok("https://hel.fi/.well-known/did.json")
        );
    }

    /// A resolver and a web server that answer what the case says.
    struct Stub {
        txt: Result<Vec<String>, Unanswered>,
        did: Result<Value, Unanswered>,
    }

    impl Lookup for Stub {
        async fn txt(&self, _name: &str) -> Result<Vec<String>, Unanswered> {
            self.txt.clone()
        }
        async fn did_document(&self, _domain: &str) -> Result<Value, Unanswered> {
            self.did.clone()
        }
    }

    const HOST: &str = "portal.hel.fi";

    fn now() -> TimestampTz {
        "2026-09-21T12:00:00Z".parse().expect("a time")
    }

    /// PF-41: the challenge is 32 random bytes, so no two Organizations share one.
    #[test]
    fn a_challenge_is_32_random_bytes_in_base64url() {
        let (one, two) = (mint_challenge(), mint_challenge());
        assert_ne!(one, two);
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&one)
            .expect("base64url");
        assert_eq!(bytes.len(), 32);
        assert_eq!(
            record("hel.fi", "abc"),
            "_joinedcontext.hel.fi TXT \"jc-verify=abc\""
        );
    }

    /// PF-41: the TXT record proves the domain; a wrong challenge, a prefix or nothing does not.
    #[tokio::test]
    async fn the_txt_record_with_the_challenge_verifies_and_a_wrong_one_fails() {
        let stub = |records: &[&str]| Stub {
            txt: Ok(records.iter().map(|r| (*r).to_owned()).collect()),
            did: Ok(Value::Null),
        };
        let verified = check(
            &stub(&["v=spf1", " jc-verify=abc "]),
            "hel.fi",
            "abc",
            HOST,
            now(),
        )
        .await;
        assert_eq!(verified.state, State::Verified);
        assert_eq!(verified.method, Some(Method::DnsTxt));
        assert_eq!(verified.checked_at, Some(now()));

        for records in [&["jc-verify=abd"][..], &["jc-verify=abcd"], &["abc"], &[]] {
            let failed = check(&stub(records), "hel.fi", "abc", HOST, now()).await;
            assert_eq!(failed.state, State::Failed, "{records:?}");
            let reason = failed.reason.expect("a reason");
            assert!(
                reason.contains("no TXT record at _joinedcontext.hel.fi"),
                "{reason}"
            );
        }
    }

    /// PF-41: a DID document that names this host in a service verifies; a host only mentioned
    /// inside another URL, or named nowhere, does not.
    #[tokio::test]
    async fn the_did_document_naming_this_host_verifies() {
        let stub = |document: Value| Stub {
            txt: Ok(Vec::new()),
            did: Ok(document),
        };
        for document in [
            json!({ "service": [{ "serviceEndpoint": "https://portal.hel.fi/api" }] }),
            json!({ "service": [{ "serviceEndpoint": ["https://x.org", "https://PORTAL.hel.fi"] }] }),
            json!({ "service": [{ "serviceEndpoint": { "origins": ["https://portal.hel.fi"] } }] }),
        ] {
            let verified = check(&stub(document.clone()), "hel.fi", "abc", HOST, now()).await;
            assert_eq!(verified.method, Some(Method::DidWeb), "{document}");
        }
        for document in [
            json!({ "service": [{ "serviceEndpoint": "https://evil.org/?portal.hel.fi" }] }),
            json!({ "service": [{ "serviceEndpoint": "https://portal.hel.fi.evil.org" }] }),
            json!({ "alsoKnownAs": ["https://portal.hel.fi"] }),
            Value::Null,
        ] {
            let failed = check(&stub(document.clone()), "hel.fi", "abc", HOST, now()).await;
            assert_eq!(failed.state, State::Failed, "{document}");
        }
    }

    /// PF-41: a lookup that did not answer is a recorded failure that says so, and never the
    /// resolver's own words.
    #[tokio::test]
    async fn an_unanswered_lookup_is_a_failure_in_words() {
        let failed = check(
            &Stub {
                txt: Err(Unanswered),
                did: Err(Unanswered),
            },
            "hel.fi",
            "abc",
            HOST,
            now(),
        )
        .await;
        assert_eq!(failed.state, State::Failed);
        assert_eq!(
            failed.reason.as_deref(),
            Some(
                "the TXT lookup of _joinedcontext.hel.fi did not answer, and \
                 https://hel.fi/.well-known/did.json could not be read"
            )
        );
    }

    /// PF-41: a verified domain is looked at again after 30 days; an unverified one after ten
    /// minutes; one never checked at once.
    #[test]
    fn a_verified_domain_is_rechecked_after_30_days() {
        let at = |state, ago: Duration| {
            DomainVerification {
                checked_at: Some(now() - jiff::SignedDuration::try_from(ago).expect("a duration")),
                ..pending("hel.fi", "abc".to_owned())
            }
            .with_state(state)
        };
        let day = Duration::from_secs(24 * 60 * 60);
        assert!(!due(&at(State::Verified, day * 29), now()));
        assert!(due(&at(State::Verified, day * 30), now()));
        assert!(!due(&at(State::Failed, Duration::from_secs(9 * 60)), now()));
        assert!(due(&at(State::Failed, Duration::from_secs(10 * 60)), now()));
        assert!(due(&pending("hel.fi", "abc".to_owned()), now()));
        // A clock that went back is not a reason to look.
        assert!(!due(
            &at(State::Verified, Duration::ZERO),
            now() - jiff::SignedDuration::from_hours(1)
        ));
    }

    impl DomainVerification {
        fn with_state(mut self, state: State) -> Self {
            self.state = state;
            self
        }
    }

    /// The status member reads as Architecture/03 §3 writes it.
    #[test]
    fn the_status_member_is_camel_case_with_dashed_methods() {
        let verified = DomainVerification {
            state: State::Verified,
            method: Some(Method::DnsTxt),
            checked_at: Some(now()),
            reason: None,
            challenge: "abc".to_owned(),
            record: record("hel.fi", "abc"),
        };
        assert_eq!(
            serde_json::to_value(&verified).expect("json"),
            json!({
                "state": "verified",
                "method": "dns-txt",
                "checkedAt": "2026-09-21T12:00:00Z",
                "challenge": "abc",
                "record": "_joinedcontext.hel.fi TXT \"jc-verify=abc\""
            })
        );
    }

    #[test]
    fn a_state_is_written_and_read_back_as_its_word() {
        for (state, word) in [
            (State::Pending, "pending"),
            (State::Verified, "verified"),
            (State::Failed, "failed"),
        ] {
            assert_eq!(state.as_str(), word);
            assert_eq!(State::parse(word), state);
        }
        assert_eq!(State::parse(""), State::Pending);
        assert_eq!(State::parse("VERIFIED"), State::Pending);
        assert_eq!(State::parse("revoked"), State::Pending);
    }

    #[test]
    fn a_method_is_written_and_read_back_as_its_word() {
        for (method, word) in [(Method::DnsTxt, "dns-txt"), (Method::DidWeb, "did-web")] {
            assert_eq!(method.as_str(), word);
            assert_eq!(Method::parse(word), Some(method));
        }
        assert_eq!(Method::parse(""), None);
        assert_eq!(Method::parse("DNS-TXT"), None);
        assert_eq!(Method::parse("email"), None);
    }

    #[test]
    fn a_service_endpoint_that_is_no_url_names_no_host() {
        for document in [
            json!({ "service": [{ "serviceEndpoint": 42 }] }),
            json!({ "service": [{ "serviceEndpoint": [null, true, 1.5] }] }),
            json!({ "service": [{ "serviceEndpoint": "not a url portal.hel.fi" }] }),
            json!({ "service": [{ "serviceEndpoint": "mailto:a@b.c" }] }),
            json!({ "service": [{}] }),
            json!({ "service": "https://portal.hel.fi" }),
            json!({ "service": [] }),
            json!([]),
        ] {
            assert!(!did_names_host(&document, HOST), "{document}");
        }
    }

    #[test]
    fn a_txt_record_is_compared_whole_after_trimming() {
        let records = |texts: &[&str]| texts.iter().map(|t| (*t).to_owned()).collect::<Vec<_>>();
        assert!(txt_proves(&records(&["\tjc-verify=abc\n"]), "abc"));
        assert!(!txt_proves(&records(&["JC-VERIFY=abc"]), "abc"));
        assert!(!txt_proves(&records(&["jc-verify=abc extra"]), "abc"));
        assert!(!txt_proves(&[], "abc"));
        assert!(!txt_proves(&records(&["jc-verify="]), "abc"));
    }

    #[tokio::test]
    async fn a_name_that_two_lookups_disagree_on_fails_with_both_reasons() {
        let stub = Stub {
            txt: Ok(Vec::new()),
            did: Err(Unanswered),
        };
        let failed = check(&stub, "hel.fi", "abc", HOST, now()).await;
        assert_eq!(
            failed.reason.as_deref(),
            Some(
                "no TXT record at _joinedcontext.hel.fi carries this instance's challenge, and \
                 https://hel.fi/.well-known/did.json could not be read"
            )
        );
        let stub = Stub {
            txt: Err(Unanswered),
            did: Ok(Value::Null),
        };
        let failed = check(&stub, "hel.fi", "abc", HOST, now()).await;
        assert_eq!(
            failed.reason.as_deref(),
            Some(
                "the TXT lookup of _joinedcontext.hel.fi did not answer, and \
                 https://hel.fi/.well-known/did.json names no service on portal.hel.fi"
            )
        );
    }

    // --- the network lookups, against a resolver of our own on loopback -----------------------

    fn name_wire(name: &str) -> Vec<u8> {
        let mut wire = Vec::new();
        for label in name.split('.') {
            wire.push(label.len() as u8);
            wire.extend_from_slice(label.as_bytes());
        }
        wire.push(0);
        wire
    }

    fn record_wire(owner: &[u8], kind: u16, data: &[u8]) -> Vec<u8> {
        let mut wire = owner.to_vec();
        wire.extend_from_slice(&kind.to_be_bytes());
        wire.extend_from_slice(&[0, 1, 0, 0, 0, 60]);
        wire.extend_from_slice(&(data.len() as u16).to_be_bytes());
        wire.extend_from_slice(data);
        wire
    }

    fn txt_data(parts: &[&str]) -> Vec<u8> {
        let mut data = Vec::new();
        for part in parts {
            data.push(part.len() as u8);
            data.extend_from_slice(part.as_bytes());
        }
        data
    }

    /// The answer section and response code for `name` and the query type (16 TXT, 1 A).
    fn zone(name: &str, kind: u16) -> (u8, Vec<Vec<u8>>) {
        const HERE: [u8; 2] = [0xC0, 0x0C];
        match (name, kind) {
            ("_joinedcontext.proves.test", 16) => (
                0,
                vec![
                    record_wire(&HERE, 16, &txt_data(&["v=spf1"])),
                    record_wire(&HERE, 16, &txt_data(&["jc-verify=", "abc"])),
                ],
            ),
            ("_joinedcontext.alias.test", 16) => (
                0,
                vec![
                    record_wire(&HERE, 5, &name_wire("target.test")),
                    record_wire(&name_wire("target.test"), 16, &txt_data(&["chained"])),
                ],
            ),
            ("host.test", 1) => (0, vec![record_wire(&HERE, 1, &[192, 0, 2, 7])]),
            (name, _) if name.starts_with("nx.") || name.contains(".nx.") => (3, Vec::new()),
            (name, _) if name.starts_with("broken.") || name.contains(".broken.") => {
                (2, Vec::new())
            }
            _ => (0, Vec::new()),
        }
    }

    fn reply(query: &[u8]) -> Option<Vec<u8>> {
        let mut at = 12;
        let mut labels = Vec::new();
        loop {
            let len = usize::from(*query.get(at)?);
            at += 1;
            if len == 0 {
                break;
            }
            labels.push(String::from_utf8_lossy(query.get(at..at + len)?).to_ascii_lowercase());
            at += len;
        }
        let kind = u16::from_be_bytes([*query.get(at)?, *query.get(at + 1)?]);
        let (rcode, answers) = zone(&labels.join("."), kind);
        let mut out = query.get(..2)?.to_vec();
        out.extend_from_slice(&[0x81, 0x80 | rcode, 0, 1]);
        out.extend_from_slice(&(answers.len() as u16).to_be_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(query.get(12..at + 4)?);
        answers.into_iter().for_each(|answer| out.extend(answer));
        Some(out)
    }

    async fn net() -> NetLookup {
        use hickory_resolver::config::{NameServerConfig, ResolverConfig};
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = socket.local_addr().expect("address").port();
        tokio::spawn(async move {
            let mut buffer = [0_u8; 1500];
            while let Ok((read, from)) = socket.recv_from(&mut buffer).await {
                if let Some(answer) = reply(&buffer[..read]) {
                    let _ = socket.send_to(&answer, from).await;
                }
            }
        });
        let mut server = NameServerConfig::udp(std::net::IpAddr::from([127, 0, 0, 1]));
        server.connections[0].port = port;
        let mut builder = hickory_resolver::TokioResolver::builder_with_config(
            ResolverConfig::from_name_servers(vec![server]),
            hickory_resolver::net::runtime::TokioRuntimeProvider::default(),
        );
        builder.options_mut().timeout = Duration::from_secs(2);
        builder.options_mut().attempts = 1;
        NetLookup {
            resolver: builder.build().expect("a resolver"),
            http: NetLookup::new().expect("the real lookups").http,
        }
    }

    #[tokio::test]
    async fn the_real_lookups_start_from_the_system_configuration() {
        assert!(NetLookup::new().is_ok());
    }

    #[tokio::test]
    async fn the_txt_strings_of_one_record_are_joined() {
        let found = Lookup::txt(&net().await, "_joinedcontext.proves.test")
            .await
            .expect("an answer");
        assert_eq!(found, ["v=spf1", "jc-verify=abc"]);
        assert!(txt_proves(&found, "abc"));
    }

    #[tokio::test]
    async fn a_cname_in_the_answer_is_not_a_txt_record() {
        let found = Lookup::txt(&net().await, "_joinedcontext.alias.test")
            .await
            .expect("an answer");
        assert_eq!(found, ["chained"]);
    }

    #[tokio::test]
    async fn a_name_with_no_txt_record_or_no_domain_answers_nothing() {
        let lookup = net().await;
        for name in ["_joinedcontext.empty.test", "_joinedcontext.nx.test"] {
            assert_eq!(Lookup::txt(&lookup, name).await, Ok(Vec::new()), "{name}");
        }
    }

    #[tokio::test]
    async fn a_resolver_that_fails_is_an_unanswered_txt_lookup() {
        let lookup = net().await;
        assert_eq!(
            Lookup::txt(&lookup, "_joinedcontext.broken.test").await,
            Err(Unanswered)
        );
    }

    #[tokio::test]
    async fn the_object_form_of_the_txt_lookup_answers_the_same() {
        let lookup = net().await;
        let found = TxtRecords::txt(&lookup, "_joinedcontext.proves.test").await;
        assert_eq!(
            found,
            Ok(vec!["v=spf1".to_owned(), "jc-verify=abc".to_owned()])
        );
        assert_eq!(
            TxtRecords::txt(&lookup, "_joinedcontext.broken.test").await,
            Err(Unanswered)
        );
    }

    #[tokio::test]
    async fn a_host_resolves_to_its_addresses_and_a_missing_one_to_none() {
        use crate::reconciler::org_hosts::Addresses;
        let lookup = net().await;
        let found = Addresses::addresses(&lookup, "host.test").await;
        assert_eq!(
            found,
            Ok(std::collections::BTreeSet::from([std::net::IpAddr::from(
                [192, 0, 2, 7]
            )]))
        );
        for name in ["nothing.test", "nx.test"] {
            assert_eq!(
                Addresses::addresses(&lookup, name).await,
                Ok(std::collections::BTreeSet::new()),
                "{name}"
            );
        }
        assert_eq!(
            Addresses::addresses(&lookup, "broken.test").await,
            Err(Unanswered)
        );
    }

    #[tokio::test]
    async fn a_did_document_of_an_internal_address_is_never_asked_for() {
        let lookup = net().await;
        for domain in ["127.0.0.1", "10.0.0.1", "localhost"] {
            assert_eq!(
                Lookup::did_document(&lookup, domain).await,
                Err(Unanswered),
                "{domain}"
            );
        }
    }

    #[tokio::test]
    async fn a_did_document_of_a_domain_that_does_not_exist_is_unanswered() {
        let lookup = net().await;
        assert_eq!(
            Lookup::did_document(&lookup, "did.nowhere.invalid").await,
            Err(Unanswered)
        );
    }

    #[test]
    fn an_unreadable_resolver_configuration_is_said_in_words() {
        let unreadable = hickory_resolver::system_conf::parse_resolv_conf(b"# no nameserver\n");
        let said = NetLookup::from_system(unreadable).err();
        assert!(
            said.as_deref()
                .is_some_and(|said| said.starts_with("the resolver configuration is unreadable")),
            "{said:?}"
        );
    }

    /// The test certificate of `tests/fixtures` (CN localhost, SAN 127.0.0.1; its key is the
    /// constant `[7; 32]`, no secret).
    const TEST_CERT: &[u8] = include_bytes!("../tests/fixtures/db-relay.cert.der");

    /// `localhost:{port}` of an https server with the test certificate that answers every
    /// request with `answer` and closes, and a lookup whose client trusts that certificate and
    /// takes `localhost` to it: the production client reaches public addresses only.
    async fn did_server(answer: Vec<u8>) -> (NetLookup, String) {
        use p256::pkcs8::EncodePrivateKey;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio_rustls::rustls;
        let key = p256::SecretKey::from_slice(&[7; 32])
            .expect("a scalar")
            .to_pkcs8_der()
            .expect("an encoded key")
            .as_bytes()
            .to_vec();
        let config = rustls::ServerConfig::builder_with_provider(std::sync::Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("TLS versions")
        .with_no_client_auth()
        .with_single_cert(
            vec![TEST_CERT.to_vec().into()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(key).into(),
        )
        .expect("the test certificate");
        let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("address").port();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                let answer = answer.clone();
                tokio::spawn(async move {
                    let mut stream = acceptor.accept(socket).await?;
                    let mut request = Vec::new();
                    let mut buffer = [0_u8; 1024];
                    while !request.windows(4).any(|end| end == b"\r\n\r\n") {
                        let read = stream.read(&mut buffer).await?;
                        if read == 0 {
                            break;
                        }
                        request.extend_from_slice(&buffer[..read]);
                    }
                    stream.write_all(&answer).await?;
                    stream.shutdown().await
                });
            }
        });
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .resolve("localhost", std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
            .add_root_certificate(
                reqwest::Certificate::from_der(TEST_CERT).expect("the test certificate"),
            )
            .timeout(Duration::from_secs(5))
            .build()
            .expect("a test client");
        (
            NetLookup {
                http,
                ..net().await
            },
            format!("localhost:{port}"),
        )
    }

    fn http_answer(status: &str, length: usize, body: &[u8]) -> Vec<u8> {
        let mut answer = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {length}\r\nconnection: close\r\n\r\n"
        )
        .into_bytes();
        answer.extend_from_slice(body);
        answer
    }

    #[tokio::test]
    async fn a_did_document_served_whole_is_read_as_json() {
        let body = br#"{"id":"did:web:hel.fi","service":[]}"#;
        let (lookup, domain) = did_server(http_answer("200 OK", body.len(), body)).await;
        assert_eq!(
            Lookup::did_document(&lookup, &domain).await,
            Ok(json!({"id": "did:web:hel.fi", "service": []}))
        );
    }

    #[tokio::test]
    async fn a_did_document_that_is_not_there_is_no_document() {
        let (lookup, domain) = did_server(http_answer("404 Not Found", 0, b"")).await;
        assert_eq!(
            Lookup::did_document(&lookup, &domain).await,
            Ok(Value::Null)
        );
    }

    #[tokio::test]
    async fn a_did_document_the_server_fails_on_is_unanswered() {
        let (lookup, domain) = did_server(http_answer("500 Internal Server Error", 0, b"")).await;
        assert_eq!(
            Lookup::did_document(&lookup, &domain).await,
            Err(Unanswered)
        );
    }

    #[tokio::test]
    async fn a_did_document_past_the_limit_is_unanswered() {
        let body = vec![b' '; DID_DOCUMENT_LIMIT + 1];
        let (lookup, domain) = did_server(http_answer("200 OK", body.len(), &body)).await;
        assert_eq!(
            Lookup::did_document(&lookup, &domain).await,
            Err(Unanswered)
        );
    }

    #[tokio::test]
    async fn a_did_document_that_is_no_json_is_unanswered() {
        let body = b"<html>not a document</html>";
        let (lookup, domain) = did_server(http_answer("200 OK", body.len(), body)).await;
        assert_eq!(
            Lookup::did_document(&lookup, &domain).await,
            Err(Unanswered)
        );
    }

    #[tokio::test]
    async fn a_did_document_cut_off_mid_body_is_unanswered() {
        let (lookup, domain) = did_server(http_answer("200 OK", 1000, br#"{"id":"#)).await;
        assert_eq!(
            Lookup::did_document(&lookup, &domain).await,
            Err(Unanswered)
        );
    }

    // --- the stored half ------------------------------------------------------------------

    fn dead_pool() -> sqlx::PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(Duration::from_millis(50))
            .connect_lazy("postgres://nobody@127.0.0.1:1/none")
            .expect("a lazy pool")
    }

    #[tokio::test]
    async fn without_a_database_every_organization_is_pending() {
        let asked = vec![
            ("alfa".to_owned(), "alfa.example".to_owned()),
            ("bravo".to_owned(), "bravo.example".to_owned()),
        ];
        let states = gate_states(None, &asked).await.expect("states");
        assert_eq!(states.len(), 2);
        assert!(states.iter().all(|gate| gate.state == State::Pending));
        assert_eq!(states[1].organization, "bravo");
        assert_eq!(states[1].domain, "bravo.example");
        assert!(gate_states(None, &[]).await.expect("none").is_empty());
    }

    #[tokio::test]
    async fn a_dead_database_is_an_error_for_the_gate_and_for_a_check() {
        let pool = dead_pool();
        let asked = vec![("alfa".to_owned(), "alfa.example".to_owned())];
        assert!(gate_states(Some(&pool), &asked).await.is_err());
        let verifier = Verifier::new(
            pool,
            Stub {
                txt: Ok(Vec::new()),
                did: Ok(Value::Null),
            },
            HOST,
        );
        assert!(verifier
            .verify("alfa", "alfa.example", now())
            .await
            .is_err());
    }
}
