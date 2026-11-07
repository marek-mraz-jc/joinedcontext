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
use chrono::{DateTime, Utc};
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
    pub checked_at: Option<DateTime<Utc>>,
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
pub fn due(verification: &DomainVerification, now: DateTime<Utc>) -> bool {
    let Some(checked) = verification.checked_at else {
        return true;
    };
    let wait = match verification.state {
        State::Verified => RECHECK_VERIFIED,
        State::Pending | State::Failed => RECHECK_UNVERIFIED,
    };
    now.signed_duration_since(checked)
        .to_std()
        .is_ok_and(|elapsed| elapsed >= wait)
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
    now: DateTime<Utc>,
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
        let mut builder = hickory_resolver::TokioResolver::builder_tokio()
            .map_err(|e| format!("the resolver configuration is unreadable: {e}"))?;
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
        now: DateTime<Utc>,
    ) -> Result<DomainVerification, sqlx::Error> {
        // `checked_at` travels as epoch seconds: the status type is chrono's, sqlx here speaks `time`.
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
                    checked_at: checked_at.and_then(|at| DateTime::from_timestamp(at, 0)),
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
        .bind(checked.checked_at.map(|at| at.timestamp() as f64))
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

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-21T12:00:00Z")
            .expect("a time")
            .with_timezone(&Utc)
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
                checked_at: Some(now() - chrono::Duration::from_std(ago).expect("a duration")),
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
            now() - chrono::Duration::hours(1)
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
}
