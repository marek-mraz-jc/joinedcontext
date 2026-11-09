//! The broker client of the `ngsi_ld` components: one call with the outbound rule, the
//! organization's request pace and the stream's `rate_limit`, the token, Bento's retry rules
//! (`internal/httpclient/client.go`: `drop_on` never retried, `backoff_on` exponential, any
//! other non-2xx linear, `successful_on` a success) plus `Retry-After`, and a 401 that
//! refreshes the token once.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use ekiben_core::{redact_url_str, CoreError, OrgContext, ResourceManager, StreamMetrics};
use ekiben_io_http::client::UnexpectedHttpStatusError;
use reqwest::header::{HeaderMap, HeaderName, ACCEPT, CONTENT_TYPE, LINK, RETRY_AFTER};
use reqwest::{Method, StatusCode};
use tokio::sync::watch;
use url::Url;

use crate::config::ClientSettings;
use crate::metrics;
use crate::outbound;
use crate::token::TokenSource;

/// The most bytes of an answer an error text repeats.
const ERROR_BODY_BYTES: usize = 2048;

/// Where a call runs: the organization, its resources, the component's metrics and label.
pub struct CallEnv<'a> {
    pub org: &'a Arc<OrgContext>,
    pub resources: &'a ResourceManager,
    pub metrics: &'a StreamMetrics,
    pub label: &'a str,
    pub shutdown: &'a watch::Receiver<bool>,
}

/// An answer of the broker that the call took (a success, or a `drop_on` refusal).
#[derive(Debug, Clone)]
pub struct Answer {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Bytes,
    /// Bento's text for this answer as an error:
    /// `<url>: HTTP request returned unexpected response code (<code>): <status>, Error: <body>`.
    pub error_text: String,
}

#[derive(Debug)]
pub enum Failure {
    /// The broker refused the request with a `drop_on` code: never retried.
    Refused(Box<Answer>),
    /// Still failing after the retries (429, 5xx, transport, another code), or stopped.
    Failed(String),
    /// Refused before any request: the outbound rule, a missing resource, a bad URL.
    Denied(CoreError),
}

impl Failure {
    pub fn text(&self) -> String {
        match self {
            Failure::Refused(a) => a.error_text.clone(),
            Failure::Failed(t) => t.clone(),
            Failure::Denied(e) => e.to_string(),
        }
    }
}

pub struct NgsiClient {
    http: reqwest::Client,
    settings: ClientSettings,
    token: Option<TokenSource>,
}

impl NgsiClient {
    pub fn new(org: &Arc<OrgContext>, settings: ClientSettings) -> Result<Self, CoreError> {
        let http = outbound::build_client(org.clone(), settings.timeout)?;
        let token = settings.oauth2.clone().map(|oa| TokenSource::new(org, oa));
        Ok(Self {
            http,
            settings,
            token,
        })
    }

    pub fn settings(&self) -> &ClientSettings {
        &self.settings
    }

    /// One request to `url` with the retries. `body` is sent as `application/json` with its
    /// `Content-Length` (Antares answers 411 without one).
    pub async fn call(
        &self,
        env: &CallEnv<'_>,
        method: Method,
        url: &str,
        body: Option<Bytes>,
    ) -> Result<Answer, Failure> {
        self.call_as(env, method, url, body, None).await
    }

    /// [`Self::call`] naming the request by `X-Request-Id` (every retry the same id), so the
    /// broker's line of the write joins the stream's.
    pub async fn call_as(
        &self,
        env: &CallEnv<'_>,
        method: Method,
        url: &str,
        body: Option<Bytes>,
        request_id: Option<&str>,
    ) -> Result<Answer, Failure> {
        let parsed = Url::parse(url).map_err(|e| {
            Failure::Denied(CoreError::config(format!(
                "invalid URL '{}': {e}",
                redact_url_str(url)
            )))
        })?;
        outbound::check_url(&parsed, env.org).map_err(Failure::Denied)?;
        let shown = redact_url_str(url);
        let retry = &self.settings.retry;
        let mut failures = 0usize;
        let mut refreshed = false;
        loop {
            self.wait_rate_limit(env).await.map_err(Failure::Denied)?;
            env.org.admit_http_request().await;
            let token = match &self.token {
                None => None,
                Some(source) => match source.token(&self.http, env.org).await {
                    Ok(t) => Some(t),
                    Err(e @ (CoreError::AccessDenied { .. } | CoreError::Config(_))) => {
                        return Err(Failure::Denied(e))
                    }
                    Err(e) => {
                        failures += 1;
                        let text = e.to_string();
                        if failures > retry.retries {
                            return Err(Failure::Failed(text));
                        }
                        tracing::warn!(
                            attempt = failures,
                            "ngsi_ld token request failed, retrying"
                        );
                        if !sleep_or_stop(env.shutdown, retry.retry_period).await {
                            return Err(Failure::Failed(format!(
                                "the stream stopped while retrying: {text}"
                            )));
                        }
                        continue;
                    }
                },
            };
            let mut req = self
                .http
                .request(method.clone(), parsed.clone())
                .header(ACCEPT, "application/json");
            for (k, v) in &self.settings.headers {
                req = req.header(k.clone(), v.clone());
            }
            if let Some(tenant) = &self.settings.tenant {
                req = req.header(HeaderName::from_static("ngsild-tenant"), tenant.clone());
            }
            if let Some(context) = &self.settings.context {
                req = req.header(
                    LINK,
                    format!(
                        "<{context}>; rel=\"http://www.w3.org/ns/json-ld#context\"; type=\"application/ld+json\""
                    ),
                );
            }
            if let Some(id) = request_id {
                req = req.header(HeaderName::from_static("x-request-id"), id);
            }
            if let Some(t) = &token {
                req = req.bearer_auth(t.expose());
            }
            if let Some(b) = &body {
                req = req.header(CONTENT_TYPE, "application/json").body(b.clone());
            }
            let started = Instant::now();
            let sent = req.send().await;
            env.metrics.record_timing(
                &metrics::latency_key(env.label),
                u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
            );
            let resp = match sent {
                Ok(resp) => resp,
                Err(e) => {
                    env.metrics
                        .incr_counter(&metrics::requests_key(env.label, "transport"), 1);
                    let text = format!(
                        "{shown}: {}",
                        redact_url_str(&outbound::error_chain(&e.without_url()))
                    );
                    failures += 1;
                    if failures > retry.retries {
                        return Err(Failure::Failed(text));
                    }
                    tracing::warn!(attempt = failures, url = %shown, "ngsi_ld request failed in transport, retrying");
                    if !sleep_or_stop(env.shutdown, retry.retry_period).await {
                        return Err(Failure::Failed(format!(
                            "the stream stopped while retrying: {text}"
                        )));
                    }
                    continue;
                }
            };
            let status = resp.status();
            let code = status.as_u16();
            env.metrics
                .incr_counter(&metrics::requests_key(env.label, &code.to_string()), 1);
            let headers = resp.headers().clone();
            let body_read = outbound::read_capped(resp, self.settings.max_response_bytes).await;
            let answer_body = match body_read {
                Ok(b) => b,
                Err(why) => {
                    let text = format!("{shown}: {why}");
                    failures += 1;
                    if failures > retry.retries {
                        return Err(Failure::Failed(text));
                    }
                    if !sleep_or_stop(env.shutdown, retry.retry_period).await {
                        return Err(Failure::Failed(format!(
                            "the stream stopped while retrying: {text}"
                        )));
                    }
                    continue;
                }
            };
            if code == 401 && !refreshed {
                if let (Some(source), Some(stale)) = (&self.token, &token) {
                    // The token expired or was revoked early: one fresh token, at once.
                    refreshed = true;
                    source.invalidate(stale).await;
                    continue;
                }
            }
            let answer = Answer {
                status: code,
                error_text: status_text(&shown, status, &answer_body),
                headers,
                body: answer_body,
            };
            if retry.drop_on.contains(&code) {
                return Err(Failure::Refused(Box::new(answer)));
            }
            let exponential = retry.backoff_on.contains(&code);
            if !exponential && (retry.successful_on.contains(&code) || status.is_success()) {
                return Ok(answer);
            }
            failures += 1;
            if failures > retry.retries {
                return Err(Failure::Failed(answer.error_text));
            }
            let backoff = if exponential {
                let shift = u32::try_from(failures.saturating_sub(1)).unwrap_or(u32::MAX);
                retry
                    .retry_period
                    .saturating_mul(2u32.saturating_pow(shift))
                    .min(retry.max_backoff)
            } else {
                retry.retry_period
            };
            let wait = retry_after(&answer.headers).map_or(backoff, |ra| ra.min(retry.max_backoff));
            tracing::warn!(status = code, attempt = failures, url = %shown, "ngsi_ld request refused, retrying");
            if !sleep_or_stop(env.shutdown, wait).await {
                return Err(Failure::Failed(format!(
                    "the stream stopped while retrying: {}",
                    answer.error_text
                )));
            }
        }
    }

    async fn wait_rate_limit(&self, env: &CallEnv<'_>) -> Result<(), CoreError> {
        let Some(name) = &self.settings.rate_limit else {
            return Ok(());
        };
        let rl = env.resources.get_rate_limit(&env.org.org_id, name)?;
        loop {
            let wait = rl.access(&[]).await?;
            if wait.is_zero() {
                return Ok(());
            }
            if !sleep_or_stop(env.shutdown, wait).await {
                return Err(CoreError::ComponentClosed);
            }
        }
    }
}

/// Sleeps `d`, or less when the stream stops; false when it stopped.
pub(crate) async fn sleep_or_stop(shutdown: &watch::Receiver<bool>, d: Duration) -> bool {
    if *shutdown.borrow() {
        return false;
    }
    let mut rx = shutdown.clone();
    tokio::select! {
        _ = tokio::time::sleep(d) => true,
        changed = rx.changed() => match changed {
            Ok(()) => !*rx.borrow(),
            Err(_) => {
                tokio::time::sleep(d).await;
                true
            }
        },
    }
}

/// `Retry-After` in seconds (an HTTP date is not honoured: the backoff applies).
fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    headers
        .get(RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
}

fn status_text(shown: &str, status: StatusCode, body: &[u8]) -> String {
    let mut text = String::from_utf8_lossy(body).into_owned();
    if text.len() > ERROR_BODY_BYTES {
        let cut = (0..=ERROR_BODY_BYTES)
            .rev()
            .find(|i| text.is_char_boundary(*i))
            .unwrap_or(0);
        text.truncate(cut);
    }
    UnexpectedHttpStatusError {
        code: status.as_u16(),
        status: format!(
            "{} {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or("Error")
        ),
        body: text,
        redacted_url: shown.to_string(),
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bento_status_text() {
        let t = status_text(
            "http://b/x",
            StatusCode::BAD_REQUEST,
            b"{\"title\":\"bad\"}\n",
        );
        assert_eq!(
            t,
            "http://b/x: HTTP request returned unexpected response code (400): 400 Bad Request, Error: {\"title\":\"bad\"}"
        );
        let long = "é".repeat(ERROR_BODY_BYTES);
        let t = status_text("u", StatusCode::BAD_GATEWAY, long.as_bytes());
        assert!(t.len() < ERROR_BODY_BYTES + 200);
    }

    #[test]
    fn retry_after_seconds_only() {
        let mut h = HeaderMap::new();
        assert_eq!(retry_after(&h), None);
        h.insert(RETRY_AFTER, "2".parse().expect("value"));
        assert_eq!(retry_after(&h), Some(Duration::from_secs(2)));
        h.insert(
            RETRY_AFTER,
            "Wed, 21 Oct 2026 07:28:00 GMT".parse().expect("value"),
        );
        assert_eq!(retry_after(&h), None);
    }
}
