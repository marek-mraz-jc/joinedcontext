//! Writes wait for a verified domain (PF-41, Architecture/03 §3, T-2572).
//!
//! The Portal checks that the Organization owns the domain it declares and keeps the state in its
//! database; the gateway reads that state from the Portal's internal listener every ten seconds
//! and, under `JC_GATEWAY_DOMAIN_VERIFICATION=enforce`, refuses a write to a space unless the
//! Organization the gateway serves is `verified`. `report`, the default, refuses nothing. The gate
//! fails closed: before the first answer, when the Portal cannot be reached, and when its list
//! does not name the Organization, the state is unknown and a write is refused. Reads never are.

use std::sync::Arc;
use std::time::{Duration, Instant};

use arc_swap::ArcSwapOption;
use jc_core::ProblemDetails;
use serde::Deserialize;

use crate::previews::WorkloadToken;

/// How often the Portal is asked, the same pace as the preview list (CC-78).
const INTERVAL: Duration = Duration::from_secs(10);
/// How old the last answer may be before the state counts as unknown: six missed polls. One
/// failed fetch does not stop every write; a Portal gone for a minute does.
pub const MAX_AGE: Duration = Duration::from_secs(60);

/// The most of the Portal's verification list the gate holds (T-5510): the gateway's body cap.
const MAX_LIST_BYTES: usize = 8 * 1024 * 1024;

/// Whether an unverified domain only shows, or also stops writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The state is recorded and shown; nothing is refused (the default).
    Report,
    /// A write to a space of an Organization that is not `verified` answers `403`.
    Enforce,
}

impl Mode {
    /// `JC_GATEWAY_DOMAIN_VERIFICATION`: unset or empty is `report`; anything but the two
    /// words is an error, so a typo never runs as the weaker setting.
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.map(str::trim) {
            None | Some("") | Some("report") => Ok(Self::Report),
            Some("enforce") => Ok(Self::Enforce),
            Some(other) => Err(format!("`{other}` is neither `report` nor `enforce`")),
        }
    }
}

/// One Organization as the Portal lists it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Item {
    /// The Organization's name.
    pub organization: String,
    /// The domain it declares, the `{orgDomain}` of its entity ids.
    pub domain: String,
    /// `pending`, `verified` or `failed`; anything but `verified` refuses under `enforce`.
    pub state: String,
}

#[derive(Deserialize)]
struct List {
    items: Vec<Item>,
}

/// The gate: its mode, and the last list the Portal answered with when it last confirmed it.
pub struct DomainGate {
    mode: Mode,
    states: ArcSwapOption<(Instant, Vec<Item>)>,
}

impl DomainGate {
    /// A gate that knows no state yet.
    pub fn new(mode: Mode) -> Self {
        Self {
            mode,
            states: ArcSwapOption::empty(),
        }
    }

    /// Takes the Portal's latest list, confirmed at `at`.
    pub fn replace_at(&self, items: Vec<Item>, at: Instant) {
        self.states.store(Some(Arc::new((at, items))));
    }

    /// The Portal answered `304`: the list stands, confirmed now.
    fn confirm(&self) {
        if let Some(current) = self.states.load_full() {
            self.replace_at(current.1.clone(), Instant::now());
        }
    }

    /// The refusal of a write to a space of the Organization whose domain is `org_domain`, or
    /// `None` when the write may go on.
    pub fn refusal(&self, org_domain: &str) -> Option<ProblemDetails> {
        self.refusal_at(org_domain, Instant::now())
    }

    /// [`Self::refusal`] for a write to a space of `project`, whose Organization declares
    /// `domain` when the repository says (S-3-14): under `enforce` a project whose
    /// Organization's domain is unknown is refused rather than judged by another's.
    pub fn refusal_for(&self, domain: Option<&str>, project: &str) -> Option<ProblemDetails> {
        match domain {
            Some(domain) => self.refusal(domain),
            None if self.mode == Mode::Report => None,
            None => Some(
                ProblemDetails::new(403, "domain-not-verified", "Forbidden").with_detail(format!(
                    "writes to the spaces of the project {project} are refused: the repository \
                     names no Organization domain for it, and this gateway refuses rather than \
                     guesses; set spec.organizationRef on the Project and spec.domain on its \
                     Organization (PF-41)"
                )),
            ),
        }
    }

    /// [`Self::refusal`] at `now`.
    pub fn refusal_at(&self, org_domain: &str, now: Instant) -> Option<ProblemDetails> {
        if self.mode == Mode::Report {
            return None;
        }
        let states = self.states.load();
        let item = states
            .as_deref()
            .filter(|(at, _)| now.saturating_duration_since(*at) <= MAX_AGE)
            .and_then(|(_, items)| items.iter().find(|item| item.domain == org_domain));
        let detail = match item {
            Some(item) if item.state == "verified" => return None,
            Some(item) => format!(
                "writes to the spaces of the organization {} are refused until its domain {} is \
                 verified (it is {}): publish the TXT record shown on the Portal's Access page, \
                 and the next check picks it up (PF-41)",
                item.organization, item.domain, item.state
            ),
            None => format!(
                "writes to the spaces of {org_domain} are refused: this gateway does not know \
                 yet whether the organization's domain is verified, and it refuses rather than \
                 guesses (PF-41)"
            ),
        };
        Some(ProblemDetails::new(403, "domain-not-verified", "Forbidden").with_detail(detail))
    }

    /// Asks the Portal for the list every ten seconds, with the gateway's own token (PF-46).
    /// A `304` confirms the list; a refusal, an error or an unreadable answer leaves it to age,
    /// so a Portal that stopped answering stops writes after [`MAX_AGE`] rather than freezing the
    /// last state.
    pub async fn follow(self: Arc<Self>, url: String, token: Option<Arc<WorkloadToken>>) {
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
        {
            Ok(client) => client,
            Err(error) => {
                tracing::error!(%error, "no HTTP client for the domain verifications");
                return;
            }
        };
        let mut ticker = tokio::time::interval(INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut etag: Option<String> = None;
        loop {
            ticker.tick().await;
            let mut request = client.get(&url);
            if let Some(token) = token.as_ref() {
                match token.get().await {
                    Ok(bearer) => request = request.bearer_auth(bearer),
                    Err(error) => {
                        tracing::warn!(%error, "no token for the Portal's domain verifications");
                        continue;
                    }
                }
            }
            if let Some(tag) = etag.as_deref() {
                request = request.header(reqwest::header::IF_NONE_MATCH, tag);
            }
            match request.send().await {
                Ok(response) if response.status() == reqwest::StatusCode::NOT_MODIFIED => {
                    self.confirm()
                }
                Ok(response) if response.status().is_success() => {
                    let tag = response
                        .headers()
                        .get(reqwest::header::ETAG)
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_owned);
                    match crate::previews::json_within::<List>(response, MAX_LIST_BYTES).await {
                        Ok(list) => {
                            self.replace_at(list.items, Instant::now());
                            etag = tag;
                        }
                        Err(error) => {
                            tracing::warn!(%error, "the domain verifications are not readable");
                            etag = None;
                        }
                    }
                }
                Ok(response) => {
                    tracing::warn!(status = %response.status(), "the Portal did not list the domain verifications");
                    etag = None;
                }
                Err(error) => {
                    tracing::warn!(%error, "the Portal did not list the domain verifications");
                    etag = None;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(state: &str) -> Item {
        Item {
            organization: "hel".into(),
            domain: "hel.fi".into(),
            state: state.into(),
        }
    }

    /// A Portal answering the scripted `(status, etag, body)` per call in turn (the last one
    /// repeats), logging each call's `If-None-Match`. `before` runs on the gate as a call
    /// arrives, before the answer goes back.
    async fn portal(
        script: Vec<(u16, Option<&'static str>, &'static str)>,
        gate: Arc<DomainGate>,
    ) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
        let asked: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let log = asked.clone();
        let app = axum::Router::new().route(
            "/internal/domain-verifications",
            axum::routing::get(move |headers: axum::http::HeaderMap| {
                let (log, gate, script) = (log.clone(), gate.clone(), script.clone());
                async move {
                    let sent = headers
                        .get(axum::http::header::IF_NONE_MATCH)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_owned();
                    let mut log = log
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let (status, etag, body) = script[log.len().min(script.len() - 1)];
                    if status == 304 {
                        // A list confirmed long ago: only the 304 makes it current again.
                        let stale = Instant::now()
                            .checked_sub(MAX_AGE * 2)
                            .unwrap_or_else(Instant::now);
                        gate.replace_at(vec![item("verified")], stale);
                    }
                    log.push(sent);
                    let mut response = axum::response::Response::builder().status(status);
                    if let Some(etag) = etag {
                        response = response.header(axum::http::header::ETAG, etag);
                    }
                    response
                        .body(axum::body::Body::from(body))
                        .unwrap_or_default()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a free port");
        let address = listener.local_addr().expect("the bound address");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (
            format!("http://{address}/internal/domain-verifications"),
            asked,
        )
    }

    /// T-7767: the follower takes a full list with its tag, sends the tag back, takes a `304`
    /// as the list confirmed now, forgets the tag after a refusal or an unreadable answer, and
    /// takes the next full list as it comes.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn the_follower_confirms_on_304_and_forgets_the_tag_after_a_failed_answer() {
        let gate = Arc::new(DomainGate::new(Mode::Enforce));
        let verified = r#"{"items":[{"organization":"hel","domain":"hel.fi","state":"verified"}]}"#;
        let failed = r#"{"items":[{"organization":"hel","domain":"hel.fi","state":"failed"}]}"#;
        let (url, asked) = portal(
            vec![
                (200, Some("\"v1\""), verified),
                (304, Some("\"v1\""), ""),
                (500, None, "down"),
                (200, Some("\"v2\""), "not json"),
                (200, None, failed),
            ],
            gate.clone(),
        )
        .await;
        let calls = || asked.lock().map(|log| log.clone()).unwrap_or_default();
        assert!(gate.refusal("hel.fi").is_some(), "no list yet refuses");
        tokio::spawn(gate.clone().follow(url, None));

        for _ in 0..600 {
            if calls().len() >= 5 && gate.refusal("hel.fi").is_some() {
                break;
            }
            if calls().len() == 2 {
                // The 304 made the stale list current again.
                for _ in 0..50 {
                    if gate.refusal("hel.fi").is_none() {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
                assert!(gate.refusal("hel.fi").is_none(), "a 304 confirms the list");
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let calls = calls();
        assert!(calls.len() >= 5, "{calls:?}");
        assert_eq!(calls[0], "", "the first call has no tag to send");
        assert_eq!(calls[1], "\"v1\"", "the full list's tag is sent back");
        assert_eq!(calls[2], "\"v1\"", "a 304 keeps the tag");
        assert_eq!(calls[3], "", "a refusal forgets the tag");
        assert_eq!(calls[4], "", "an unreadable list forgets the tag");
        assert!(
            gate.refusal("hel.fi").is_some(),
            "the last full list says failed and is taken"
        );
    }

    /// T-7767: a `304` before any list leaves the gate without one, so it still refuses.
    #[test]
    fn a_confirmation_without_a_list_confirms_nothing() {
        let gate = DomainGate::new(Mode::Enforce);
        gate.confirm();
        assert!(gate.refusal("hel.fi").is_some());
    }

    /// PF-41: `report` refuses nothing whatever the state, and the setting admits only its two
    /// words, so a typo never runs as the weaker one.
    #[test]
    fn report_refuses_nothing_and_the_setting_is_one_of_two_words() {
        let gate = DomainGate::new(Mode::Report);
        assert!(gate.refusal("hel.fi").is_none());
        gate.replace_at(vec![item("failed")], Instant::now());
        assert!(gate.refusal("hel.fi").is_none());

        assert_eq!(Mode::parse(None), Ok(Mode::Report));
        assert_eq!(Mode::parse(Some("")), Ok(Mode::Report));
        assert_eq!(Mode::parse(Some("enforce")), Ok(Mode::Enforce));
        assert!(Mode::parse(Some("enforced")).is_err());
        assert!(Mode::parse(Some("off")).is_err());
    }

    /// PF-41: under `enforce` only a verified domain lets a write through; pending, failed, an
    /// Organization the list does not name, no list yet and a list older than a minute are all
    /// refused, with the Organization named.
    #[test]
    fn enforce_lets_a_write_through_only_for_a_verified_domain_and_fails_closed() {
        let gate = DomainGate::new(Mode::Enforce);
        let now = Instant::now();
        let unknown = gate
            .refusal_at("hel.fi", now)
            .expect("refused before the first answer");
        assert_eq!(unknown.status, 403);
        assert!(unknown
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("hel.fi"));

        gate.replace_at(vec![item("verified")], now);
        assert!(gate.refusal_at("hel.fi", now).is_none());
        assert!(
            gate.refusal_at("espoo.fi", now).is_some(),
            "another domain is not this one"
        );

        for state in ["pending", "failed"] {
            gate.replace_at(vec![item(state)], now);
            let refused = gate.refusal_at("hel.fi", now).expect("refused");
            assert_eq!(refused.status, 403);
            let detail = refused.detail.unwrap_or_default();
            assert!(
                detail.contains("organization hel") && detail.contains(state),
                "{detail}"
            );
            assert!(refused.type_uri.ends_with("/domain-not-verified"));
        }

        // A verified answer holds through a missed poll and stops holding after a minute.
        gate.replace_at(vec![item("verified")], now);
        assert!(gate
            .refusal_at("hel.fi", now + Duration::from_secs(25))
            .is_none());
        assert!(
            gate.refusal_at("hel.fi", now + MAX_AGE + Duration::from_secs(1))
                .is_some(),
            "a Portal gone for a minute stops writes"
        );
    }
}
