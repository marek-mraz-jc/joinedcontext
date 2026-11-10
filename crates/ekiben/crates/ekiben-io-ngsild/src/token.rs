//! Client-credentials tokens for the `ngsi_ld` components: the client's id and secret in a Basic
//! header first and in the form when the endpoint refuses that (x/oauth2's AuthStyleAutoDetect,
//! as ekiben-io-http does for Bento parity), cached per organization, token URL and client,
//! refreshed [`REFRESH_AHEAD`] before `expires_in`, fetched by one caller at a time (single
//! flight), and after a failed fetch not asked again until its [`TokenBackoff`] wait is over. No
//! token or secret is ever logged or put into an error text.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use base64::Engine as _;
use ekiben_core::token_backoff::retry_after;
use ekiben_core::{redact_url_str, CoreError, OrgContext, SecretString, TokenBackoff};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, RETRY_AFTER};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex as AsyncMutex;
use url::Url;

use crate::config::OAuth2Settings;
use crate::outbound;

/// How long before `expires_in` a token is refreshed, at most; half its life when that is shorter.
pub const REFRESH_AHEAD: Duration = Duration::from_secs(30);
/// The life of a token whose answer names no `expires_in`; a 401 refreshes it earlier.
const DEFAULT_LIFE: Duration = Duration::from_secs(3600);
/// The most clients whose tokens one process caches; idle ones are evicted first.
pub const MAX_CACHED_CLIENTS: usize = 1024;
/// The most bytes a token answer may have.
const MAX_TOKEN_ANSWER: usize = 64 * 1024;

#[derive(Clone, PartialEq, Eq, Hash)]
struct TokenKey {
    org: String,
    token_url: String,
    client: String,
    secret_digest: String,
}

struct Cached {
    token: SecretString,
    refresh_at: Instant,
}

#[derive(Default)]
struct Held {
    cached: Option<Cached>,
    backoff: TokenBackoff,
}

type Slot = Arc<AsyncMutex<Held>>;

fn slots() -> &'static Mutex<HashMap<TokenKey, Slot>> {
    static SLOTS: OnceLock<Mutex<HashMap<TokenKey, Slot>>> = OnceLock::new();
    SLOTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The cache slot of `key`, shared by every component of the organization naming the same
/// client. A slot no component holds any more is evicted when the cache is full.
fn slot_for(key: &TokenKey) -> Slot {
    let mut map = slots().lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(s) = map.get(key) {
        return s.clone();
    }
    if map.len() >= MAX_CACHED_CLIENTS {
        map.retain(|_, s| Arc::strong_count(s) > 1);
    }
    let slot: Slot = Arc::default();
    if map.len() < MAX_CACHED_CLIENTS {
        map.insert(key.clone(), slot.clone());
    }
    slot
}

/// The token of one configured client for one organization.
pub struct TokenSource {
    settings: OAuth2Settings,
    slot: Slot,
}

impl std::fmt::Debug for TokenSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenSource")
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

impl TokenSource {
    pub fn new(org: &OrgContext, settings: OAuth2Settings) -> Self {
        let digest = Sha256::digest(settings.client_secret.expose().as_bytes());
        let key = TokenKey {
            org: org.org_id.0.clone(),
            token_url: settings.token_url.clone(),
            client: settings.client_key.clone(),
            secret_digest: hex::encode(&digest[..]),
        };
        Self {
            slot: slot_for(&key),
            settings,
        }
    }

    /// A token valid for at least a moment more: the cached one, or a fresh one.
    pub async fn token(
        &self,
        http: &reqwest::Client,
        org: &OrgContext,
    ) -> Result<SecretString, CoreError> {
        let mut guard = self.slot.lock().await;
        if let Some(cached) = guard.cached.as_ref() {
            if Instant::now() < cached.refresh_at {
                return Ok(cached.token.clone());
            }
        }
        if let Some(left) = guard.backoff.wait() {
            return Err(CoreError::other(format!(
                "OAuth2 token request held back {} s after the token endpoint failed",
                left.as_secs().max(1)
            )));
        }
        match self.fetch(http, org).await {
            Ok(fresh) => {
                let token = fresh.token.clone();
                guard.cached = Some(fresh);
                guard.backoff.succeeded();
                Ok(token)
            }
            Err((error, asked)) => {
                guard.backoff.failed(asked);
                Err(error)
            }
        }
    }

    /// Drops the cached token when it is still `stale` (one the broker answered 401 to), so the
    /// next caller fetches a new one and concurrent callers do not each refetch.
    pub async fn invalidate(&self, stale: &SecretString) {
        let mut guard = self.slot.lock().await;
        if guard.cached.as_ref().is_some_and(|c| c.token == *stale) {
            guard.cached = None;
        }
    }

    /// A fresh token, or the error with the endpoint's `Retry-After`.
    async fn fetch(
        &self,
        http: &reqwest::Client,
        org: &OrgContext,
    ) -> Result<Cached, (CoreError, Option<Duration>)> {
        let s = &self.settings;
        let url = Url::parse(&s.token_url)
            .map_err(|_| {
                CoreError::config(format!(
                    "oauth2.token_url {:?} is not a URL",
                    redact_url_str(&s.token_url)
                ))
            })
            .map_err(|e| (e, None))?;
        outbound::check_url(&url, org).map_err(|e| (e, None))?;
        org.admit_http_request().await;
        let mut resp = http
            .post(url.clone())
            .header(AUTHORIZATION, basic(s))
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(form(s, false))
            .send()
            .await
            .map_err(failed)?;
        // Only a refusal of the header style is asked again in the form; a 429 or a 5xx is the
        // endpoint's state, and a second request would only add to it.
        if refuses_the_style(resp.status()) {
            org.admit_http_request().await;
            resp = http
                .post(url)
                .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(form(s, true))
                .send()
                .await
                .map_err(failed)?;
        }
        let status = resp.status();
        if !status.is_success() {
            let asked = retry_after(
                resp.headers()
                    .get(RETRY_AFTER)
                    .and_then(|v| v.to_str().ok()),
            );
            return Err((
                CoreError::other(format!(
                    "OAuth2 token request failed: the token endpoint answered {status}"
                )),
                asked,
            ));
        }
        let body = outbound::read_capped(resp, MAX_TOKEN_ANSWER)
            .await
            .map_err(|e| {
                (
                    CoreError::other(format!("OAuth2 token request failed: {e}")),
                    None,
                )
            })?;
        let (token, expires_in) = parse_token_answer(&body).ok_or_else(|| {
            (
                CoreError::other("OAuth2 token request failed: the answer carries no access_token"),
                None,
            )
        })?;
        let life = expires_in.map_or(DEFAULT_LIFE, Duration::from_secs);
        let ahead = REFRESH_AHEAD.min(life / 2);
        Ok(Cached {
            token: SecretString::new(token),
            refresh_at: Instant::now() + life.saturating_sub(ahead),
        })
    }
}

fn failed(e: reqwest::Error) -> (CoreError, Option<Duration>) {
    let error = CoreError::other(format!(
        "OAuth2 token request failed: {}",
        redact_url_str(&outbound::error_chain(&e.without_url()))
    ));
    (error, None)
}

/// Whether `status` refuses the client's credentials in the header style, so x/oauth2's
/// auto-detection asks again with them in the form: a 4xx other than 429.
pub(crate) fn refuses_the_style(status: reqwest::StatusCode) -> bool {
    status.is_client_error() && status != reqwest::StatusCode::TOO_MANY_REQUESTS
}

/// `access_token` and `expires_in` of a JSON answer, or of a form-encoded one (x/oauth2 reads
/// both; Bento's own test token endpoint answers a form).
pub(crate) fn parse_token_answer(body: &[u8]) -> Option<(String, Option<u64>)> {
    if let Ok(JsonValue::Object(m)) = serde_json::from_slice::<JsonValue>(body) {
        let token = m.get("access_token")?.as_str()?.to_string();
        let expires = m.get("expires_in").and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
        });
        return (!token.is_empty()).then_some((token, expires));
    }
    let mut token = None;
    let mut expires = None;
    for (k, v) in url::form_urlencoded::parse(body) {
        match k.as_ref() {
            "access_token" => token = Some(v.into_owned()),
            "expires_in" => expires = v.trim().parse().ok(),
            _ => {}
        }
    }
    token.filter(|t| !t.is_empty()).map(|t| (t, expires))
}

/// The client-credentials form: `grant_type`, `scope` when scopes are set, the
/// `endpoint_params`, and with `in_params` the client's id and secret; keys sorted, as Go's
/// `url.Values.Encode` writes them.
pub(crate) fn form(oa: &OAuth2Settings, in_params: bool) -> String {
    let mut values: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    values.insert("grant_type".into(), vec!["client_credentials".into()]);
    if !oa.scopes.is_empty() {
        values.insert("scope".into(), vec![oa.scopes.join(" ")]);
    }
    for (k, v) in &oa.endpoint_params {
        values.insert(k.clone(), v.clone());
    }
    if in_params {
        values.insert("client_id".into(), vec![oa.client_key.clone()]);
        values.insert(
            "client_secret".into(),
            vec![oa.client_secret.expose().to_string()],
        );
    }
    let mut out = url::form_urlencoded::Serializer::new(String::new());
    for (k, vs) in &values {
        for v in vs {
            out.append_pair(k, v);
        }
    }
    out.finish()
}

/// `Basic base64(QueryEscape(id) ":" QueryEscape(secret))`, x/oauth2's AuthStyleInHeader. The
/// token sidecar reads the `{project}/{pipeline}` client id from it and wants no secret.
pub(crate) fn basic(oa: &OAuth2Settings) -> String {
    let escape = |v: &str| url::form_urlencoded::byte_serialize(v.as_bytes()).collect::<String>();
    let pair = format!(
        "{}:{}",
        escape(&oa.client_key),
        escape(oa.client_secret.expose())
    );
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(pair)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn settings(key: &str, secret: &str) -> OAuth2Settings {
        OAuth2Settings {
            client_key: key.into(),
            client_secret: SecretString::new(secret),
            token_url: "http://t/token".into(),
            scopes: vec![],
            endpoint_params: BTreeMap::new(),
        }
    }

    #[test]
    fn basic_and_form_match_x_oauth2() {
        let s = settings("fookey", "foosecret");
        assert_eq!(basic(&s), "Basic Zm9va2V5OmZvb3NlY3JldA==");
        assert_eq!(form(&s, false), "grant_type=client_credentials");
        assert_eq!(
            form(&s, true),
            "client_id=fookey&client_secret=foosecret&grant_type=client_credentials"
        );
        let sidecar = settings("helsinki/departures", "");
        // QueryEscape("helsinki/departures") + ":" with no secret, as the sidecar decodes it.
        let expected = base64::engine::general_purpose::STANDARD.encode("helsinki%2Fdepartures:");
        assert_eq!(basic(&sidecar), format!("Basic {expected}"));
    }

    #[test]
    fn token_answers_in_json_and_form() {
        assert_eq!(
            parse_token_answer(br#"{"access_token":"a","expires_in":60}"#),
            Some(("a".into(), Some(60)))
        );
        assert_eq!(
            parse_token_answer(br#"{"access_token":"a","expires_in":"60"}"#),
            Some(("a".into(), Some(60)))
        );
        assert_eq!(
            parse_token_answer(b"access_token=footoken&token_type=Bearer"),
            Some(("footoken".into(), None))
        );
        assert_eq!(parse_token_answer(br#"{"token":"a"}"#), None);
        assert_eq!(parse_token_answer(br#"{"access_token":""}"#), None);
        assert_eq!(parse_token_answer(b""), None);
    }

    #[test]
    fn the_cache_is_bounded_and_keeps_slots_in_use() {
        let org = OrgContext::new(
            ekiben_core::OrgId::new("bounded"),
            ekiben_core::OrgQuotas::default(),
        );
        let held = TokenSource::new(&org, settings("held", "s"));
        for i in 0..(MAX_CACHED_CLIENTS + 10) {
            let _ = TokenSource::new(&org, settings(&format!("c{i}"), "s"));
        }
        let len = slots().lock().unwrap_or_else(PoisonError::into_inner).len();
        assert!(len <= MAX_CACHED_CLIENTS, "{len}");
        let again = TokenSource::new(&org, settings("held", "s"));
        assert!(Arc::ptr_eq(&held.slot, &again.slot));
    }
}
