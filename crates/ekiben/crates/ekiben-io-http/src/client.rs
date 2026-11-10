use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use ekiben_bloblang::{Environment, FieldExpression, Value};
use ekiben_core::config::{extract_metadata_filter, parse_duration_str, MetadataFilter};
use ekiben_core::{
    redact_url_str, Batch, CoreError, OrgContext, Part, PartsView, ResourceManager, SecretString,
};
use futures::StreamExt;
use reqwest::redirect::Policy as RedirectPolicy;
use reqwest::Method;
use serde_json::Value as JsonValue;
use tokio::sync::Mutex as AsyncMutex;
use url::Url;

pub const DEFAULT_MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024; // 64 MiB
pub const MAX_REDIRECTS: usize = 5;

#[derive(Debug, Clone)]
pub struct UnexpectedHttpStatusError {
    pub code: u16,
    pub status: String,
    pub body: String,
    pub redacted_url: String,
}

impl std::fmt::Display for UnexpectedHttpStatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let clean_body = self.body.replace('\n', "");
        write!(
            f,
            "{}: HTTP request returned unexpected response code ({}): {}, Error: {}",
            self.redacted_url, self.code, self.status, clean_body
        )
    }
}

impl std::error::Error for UnexpectedHttpStatusError {}

/// Go's `Response.Status`: the code and the phrase the server sent (hyper keeps one that is not
/// the canonical phrase, an empty one included, as `ReasonPhrase`), else the canonical phrase.
pub fn go_status(resp: &reqwest::Response) -> String {
    let phrase = match resp.extensions().get::<hyper::ext::ReasonPhrase>() {
        Some(sent) => String::from_utf8_lossy(sent.as_bytes()).into_owned(),
        None => resp.status().canonical_reason().unwrap_or("").to_string(),
    };
    format!("{} {phrase}", resp.status().as_u16())
}

/// The form of a client-credentials token request as x/oauth2 encodes it: `grant_type`, `scope`
/// when scopes are set, the `endpoint_params` over both, and with `in_params` the client's id and
/// secret; keys sorted, as Go's `url.Values.Encode` writes them.
fn oauth2_form(oa: &OAuth2Config, in_params: bool) -> String {
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
    let mut form = url::form_urlencoded::Serializer::new(String::new());
    for (k, vs) in &values {
        for v in vs {
            form.append_pair(k, v);
        }
    }
    form.finish()
}

/// `Basic base64(QueryEscape(id) ":" QueryEscape(secret))`, x/oauth2's AuthStyleInHeader.
fn oauth2_basic(oa: &OAuth2Config) -> String {
    use base64::Engine as _;
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

fn oauth2_failed(e: &reqwest::Error) -> CoreError {
    CoreError::other(format!(
        "OAuth2 token request failed: {}",
        redact_url_str(&error_chain(e))
    ))
}

#[derive(Clone, Debug)]
pub struct OAuth2Config {
    pub enabled: bool,
    pub client_key: String,
    pub client_secret: SecretString,
    pub token_url: String,
    pub scopes: Vec<String>,
    pub endpoint_params: HashMap<String, Vec<String>>,
}

#[derive(Clone, Debug)]
pub struct BasicAuthConfig {
    pub enabled: bool,
    pub username: String,
    pub password: SecretString,
}

#[derive(Clone, Debug)]
pub struct MultipartExpr {
    pub content_type: FieldExpression,
    pub content_disposition: FieldExpression,
    pub body: FieldExpression,
}

struct CachedOAuthToken {
    access_token: SecretString,
    expires_at: Instant,
}

/// Shared HTTP client matching `internal/httpclient/client.go` with strict security controls.
pub struct HttpClient {
    /// Refuses private, loopback and link-local peers, also after DNS and on redirects.
    http: reqwest::Client,
    /// For organizations whose quota sets `allow_private_ips`.
    http_open: reqwest::Client,
    /// For the hosting process's own listener ([`ekiben_core::HostTarget`]): follows no redirect,
    /// so the listener cannot send a stream on to another mount or address.
    http_host: reqwest::Client,
    url: FieldExpression,
    raw_url: String,
    verb: Method,
    headers: HashMap<String, FieldExpression>,
    meta_insert: MetadataFilter,
    meta_extract: MetadataFilter,
    rate_limit: Option<String>,
    retry_period: Duration,
    max_backoff: Duration,
    num_retries: usize,
    backoff_on: HashSet<u16>,
    drop_on: HashSet<u16>,
    successful_on: HashSet<u16>,
    payload: Option<FieldExpression>,
    multipart: Vec<MultipartExpr>,
    oauth2: Option<OAuth2Config>,
    basic_auth: Option<BasicAuthConfig>,
    max_response_bytes: usize,
    cached_token: AsyncMutex<Option<CachedOAuthToken>>,
    /// This client's id: a call to the host's own listener is `X-Request-Id: {run}-<n>`, so the
    /// gateway's and the broker's lines for a stream's write carry the stream's reference.
    run: String,
    calls: AtomicU64,
    /// Whether the line naming `run` was written (once, at the first answered call).
    named: AtomicBool,
}

impl HttpClient {
    fn client_for(&self, org: &OrgContext, url: &Url) -> &reqwest::Client {
        if org.get_quotas().allow_private_ips {
            &self.http_open
        } else if org.host_target().is_some_and(|t| t.admits(url)) {
            &self.http_host
        } else {
            &self.http
        }
    }

    pub fn from_json_config(
        conf: &JsonValue,
        default_verb: &str,
        blobl_env: &Environment,
    ) -> Result<Arc<Self>, CoreError> {
        let obj = conf
            .as_object()
            .ok_or_else(|| CoreError::config("http client config must be an object"))?;

        let raw_url = obj
            .get("url")
            .and_then(JsonValue::as_str)
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| CoreError::config("http client requires a non-empty 'url' field"))?
            .to_string();

        // Validate static scheme prefix at config time (must start with http:// or https://)
        validate_url_scheme_prefix(&raw_url)?;

        let url = blobl_env.compile_field(&raw_url)?;
        let verb_str = obj
            .get("verb")
            .and_then(JsonValue::as_str)
            .unwrap_or(default_verb)
            .to_ascii_uppercase();
        let verb = Method::from_bytes(verb_str.as_bytes())
            .map_err(|e| CoreError::config(format!("invalid HTTP verb '{verb_str}': {e}")))?;

        let mut headers = HashMap::new();
        if let Some(h_map) = obj.get("headers").and_then(JsonValue::as_object) {
            for (k, v) in h_map {
                if let Some(s) = v.as_str() {
                    headers.insert(k.clone(), blobl_env.compile_field(s)?);
                }
            }
        }

        let meta_insert = obj
            .get("metadata")
            .map(extract_metadata_filter)
            .unwrap_or_default();
        let meta_extract = obj
            .get("extract_headers")
            .map(extract_metadata_filter)
            .unwrap_or_default();

        let rate_limit = obj
            .get("rate_limit")
            .and_then(JsonValue::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from);

        let timeout = match obj.get("timeout").and_then(JsonValue::as_str) {
            Some(s) => parse_duration_str(s)?,
            None => Duration::from_secs(5),
        };
        let retry_period = match obj.get("retry_period").and_then(JsonValue::as_str) {
            Some(s) => parse_duration_str(s)?,
            None => Duration::from_secs(1),
        };
        let max_backoff = match obj.get("max_retry_backoff").and_then(JsonValue::as_str) {
            Some(s) => parse_duration_str(s)?,
            None => Duration::from_secs(300),
        };
        let num_retries = obj.get("retries").and_then(JsonValue::as_u64).unwrap_or(3) as usize;

        let parse_code_set = |key: &str, default: &[u16]| -> HashSet<u16> {
            match obj.get(key).and_then(JsonValue::as_array) {
                Some(arr) => arr
                    .iter()
                    .filter_map(JsonValue::as_u64)
                    .map(|c| c as u16)
                    .collect(),
                None => default.iter().copied().collect(),
            }
        };
        let backoff_on = parse_code_set("backoff_on", &[429]);
        let drop_on = parse_code_set("drop_on", &[]);
        let successful_on = parse_code_set("successful_on", &[]);

        let payload = match obj.get("payload").and_then(JsonValue::as_str) {
            Some(s) if !s.is_empty() => Some(blobl_env.compile_field(s)?),
            _ => None,
        };

        let mut multipart = Vec::new();
        if let Some(mp_arr) = obj.get("multipart").and_then(JsonValue::as_array) {
            for item in mp_arr {
                if let Some(m) = item.as_object() {
                    let ct = m
                        .get("content_type")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("");
                    let cd = m
                        .get("content_disposition")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("");
                    let b = m.get("body").and_then(JsonValue::as_str).unwrap_or("");
                    multipart.push(MultipartExpr {
                        content_type: blobl_env.compile_field(ct)?,
                        content_disposition: blobl_env.compile_field(cd)?,
                        body: blobl_env.compile_field(b)?,
                    });
                }
            }
        }

        let oauth2 = if let Some(oa) = obj.get("oauth2").and_then(JsonValue::as_object) {
            let enabled = oa
                .get("enabled")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false);
            if enabled {
                let token_url = oa
                    .get("token_url")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("")
                    .to_string();
                validate_url_scheme_prefix(&token_url)?;
                let client_key = oa
                    .get("client_key")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("")
                    .to_string();
                let client_secret = SecretString::new(
                    oa.get("client_secret")
                        .and_then(JsonValue::as_str)
                        .unwrap_or(""),
                );
                let scopes = oa
                    .get("scopes")
                    .and_then(JsonValue::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(JsonValue::as_str)
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default();
                Some(OAuth2Config {
                    enabled,
                    client_key,
                    client_secret,
                    token_url,
                    scopes,
                    endpoint_params: HashMap::new(),
                })
            } else {
                None
            }
        } else {
            None
        };

        let basic_auth = if let Some(ba) = obj.get("basic_auth").and_then(JsonValue::as_object) {
            let enabled = ba
                .get("enabled")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false);
            if enabled {
                Some(BasicAuthConfig {
                    enabled,
                    username: ba
                        .get("username")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("")
                        .to_string(),
                    password: SecretString::new(
                        ba.get("password").and_then(JsonValue::as_str).unwrap_or(""),
                    ),
                })
            } else {
                None
            }
        } else {
            None
        };

        let max_response_bytes = obj
            .get("max_response_bytes")
            .and_then(JsonValue::as_u64)
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_MAX_RESPONSE_BYTES);

        let http = build_http(timeout, true)?;
        let http_open = build_http(timeout, false)?;
        let http_host = build_host_http(timeout)?;

        Ok(Arc::new(Self {
            http,
            http_open,
            http_host,
            url,
            raw_url,
            verb,
            headers,
            meta_insert,
            meta_extract,
            rate_limit,
            retry_period,
            max_backoff,
            num_retries,
            backoff_on,
            drop_on,
            successful_on,
            payload,
            multipart,
            oauth2,
            basic_auth,
            max_response_bytes,
            cached_token: AsyncMutex::new(None),
            run: uuid::Uuid::now_v7().simple().to_string(),
            calls: AtomicU64::new(0),
            named: AtomicBool::new(false),
        }))
    }

    pub fn raw_url(&self) -> &str {
        &self.raw_url
    }

    fn check_status(&self, code: u16) -> (bool, RetryStrategy) {
        if self.drop_on.contains(&code) {
            return (false, RetryStrategy::NoRetry);
        }
        if self.backoff_on.contains(&code) {
            return (false, RetryStrategy::Exponential);
        }
        if self.successful_on.contains(&code) {
            return (true, RetryStrategy::NoRetry);
        }
        if !(200..=299).contains(&code) {
            return (false, RetryStrategy::Linear);
        }
        (true, RetryStrategy::NoRetry)
    }

    async fn wait_rate_limit(
        &self,
        org: &OrgContext,
        resources: &ResourceManager,
        batch: &[Part],
    ) -> Result<(), CoreError> {
        let Some(ref rl_name) = self.rate_limit else {
            return Ok(());
        };
        let rl = resources.get_rate_limit(&org.org_id, rl_name)?;
        loop {
            let wait = rl.access(batch).await?;
            if wait.is_zero() {
                return Ok(());
            }
            tokio::time::sleep(wait).await;
        }
    }

    async fn fetch_oauth2_token(
        &self,
        oa: &OAuth2Config,
        org: &OrgContext,
    ) -> Result<String, CoreError> {
        {
            let guard = self.cached_token.lock().await;
            if let Some(ref cached) = *guard {
                if Instant::now() < cached.expires_at {
                    return Ok(cached.access_token.expose().to_string());
                }
            }
        }

        let parsed_token_url = validate_resolved_url(&oa.token_url, org)?;
        org.admit_http_request().await;
        // As golang.org/x/oauth2 under Bento (AuthStyleAutoDetect): the client's credentials in a
        // Basic header first, and only when the endpoint refuses that, in the form instead.
        let mut resp = self
            .client_for(org, &parsed_token_url)
            .post(parsed_token_url.clone())
            .header("Authorization", oauth2_basic(oa))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(oauth2_form(oa, false))
            .send()
            .await
            .map_err(|e| oauth2_failed(&e))?;
        if !resp.status().is_success() {
            resp = self
                .client_for(org, &parsed_token_url)
                .post(parsed_token_url)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(oauth2_form(oa, true))
                .send()
                .await
                .map_err(|e| oauth2_failed(&e))?;
        }

        if !resp.status().is_success() {
            return Err(CoreError::other(format!(
                "OAuth2 token endpoint returned status {}",
                resp.status()
            )));
        }

        let body_json: JsonValue = resp
            .json()
            .await
            .map_err(|e| CoreError::other(format!("invalid OAuth2 token response: {e}")))?;
        let token_str = body_json
            .get("access_token")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| CoreError::other("OAuth2 response missing access_token"))?
            .to_string();
        let expires_in = body_json
            .get("expires_in")
            .and_then(JsonValue::as_u64)
            .unwrap_or(3600);

        let mut guard = self.cached_token.lock().await;
        *guard = Some(CachedOAuthToken {
            access_token: SecretString::new(token_str.clone()),
            expires_at: Instant::now() + Duration::from_secs(expires_in.saturating_sub(10).max(1)),
        });
        Ok(token_str)
    }

    async fn build_request(
        &self,
        ref_batch: &[Part],
        batch_as_multipart: bool,
        org: &OrgContext,
    ) -> Result<(reqwest::RequestBuilder, String, Option<String>), CoreError> {
        let view = PartsView(ref_batch);
        let resolved_url_str = self
            .url
            .resolve_string(0, &view)
            .map_err(|e| CoreError::other(format!("url interpolation error: {e}")))?;

        let parsed_url = validate_resolved_url(&resolved_url_str, org)?;
        org.admit_http_request().await;
        let redacted_url = redact_url_str(parsed_url.as_str());
        let own = org.host_target().is_some_and(|t| t.admits(&parsed_url))
            && !self
                .headers
                .keys()
                .any(|k| k.eq_ignore_ascii_case("x-request-id"));

        let mut req = self
            .client_for(org, &parsed_url)
            .request(self.verb.clone(), parsed_url);
        // 32 hex digits, a dash, digits: a plain token jc's door keeps as the caller's id.
        let request_id = own.then(|| {
            let n = self.calls.fetch_add(1, Ordering::Relaxed) + 1;
            format!("{}-{n}", self.run)
        });
        if let Some(id) = &request_id {
            req = req.header("X-Request-Id", id);
        }

        let mut has_content_type = false;
        for (k, expr) in &self.headers {
            let val = expr
                .resolve_string(0, &view)
                .map_err(|e| CoreError::other(format!("header '{k}' interpolation error: {e}")))?;
            if k.eq_ignore_ascii_case("content-type") {
                has_content_type = true;
            }
            req = req.header(k, val);
        }

        if let Some(first) = ref_batch.first() {
            for (k, v) in first.meta_all() {
                if self.meta_insert.matches(k) {
                    req = req.header(k, v.to_string_coerced());
                }
            }
        }

        if let Some(ref ba) = self.basic_auth {
            if ba.enabled {
                req = req.basic_auth(&ba.username, Some(ba.password.expose()));
            }
        }

        if let Some(ref oa) = self.oauth2 {
            if oa.enabled {
                let token = self.fetch_oauth2_token(oa, org).await?;
                req = req.bearer_auth(token);
            }
        }

        // Body construction
        if let Some(ref payload_expr) = self.payload {
            let body_bytes = payload_expr
                .resolve_bytes(0, &view)
                .map_err(|e| CoreError::other(format!("payload interpolation error: {e}")))?;
            if !has_content_type {
                req = req.header("Content-Type", "application/octet-stream");
            }
            req = req.body(body_bytes);
        } else if !self.multipart.is_empty() {
            let boundary = "ekiben-multipart-boundary-7f8a9b";
            let mut body_buf = Vec::new();
            for mp in &self.multipart {
                let ct = mp.content_type.resolve_string(0, &view).unwrap_or_default();
                let cd = mp
                    .content_disposition
                    .resolve_string(0, &view)
                    .unwrap_or_default();
                let part_bytes = mp.body.resolve_bytes(0, &view).unwrap_or_default();
                body_buf.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
                if !ct.is_empty() {
                    body_buf.extend_from_slice(format!("Content-Type: {ct}\r\n").as_bytes());
                }
                if !cd.is_empty() {
                    body_buf.extend_from_slice(format!("Content-Disposition: {cd}\r\n").as_bytes());
                }
                body_buf.extend_from_slice(b"\r\n");
                body_buf.extend_from_slice(&part_bytes);
                body_buf.extend_from_slice(b"\r\n");
            }
            body_buf.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
            req = req
                .header(
                    "Content-Type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(body_buf);
        } else if ref_batch.len() == 1 {
            // Bento names the type of a one-message body even when it is empty, a GET's too
            // (internal/httpclient/request.go `body`).
            if !has_content_type {
                req = req.header("Content-Type", "application/octet-stream");
            }
            let body_bytes = ref_batch[0].as_bytes_ref();
            if !body_bytes.is_empty()
                || matches!(self.verb, Method::POST | Method::PUT | Method::PATCH)
            {
                req = req.body(body_bytes);
            }
        } else if ref_batch.len() > 1 && batch_as_multipart {
            let boundary = "ekiben-batch-boundary-3c4d5e";
            let mut body_buf = Vec::new();
            for part in ref_batch {
                body_buf.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
                body_buf.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
                body_buf.extend_from_slice(&part.as_bytes_ref());
                body_buf.extend_from_slice(b"\r\n");
            }
            body_buf.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
            req = req
                .header(
                    "Content-Type",
                    format!("multipart/mixed; boundary={boundary}"),
                )
                .body(body_buf);
        }

        Ok((req, redacted_url, request_id))
    }

    /// Executes an HTTP request with retries, rate limiting, and response size cap.
    pub async fn send(
        &self,
        mut send_batch: Batch,
        batch_as_multipart: bool,
        org: &OrgContext,
        resources: &ResourceManager,
    ) -> Result<Batch, CoreError> {
        for p in &mut send_batch {
            let _ = p.as_bytes();
        }

        let mut attempt = 0usize;
        let max_attempts = 1 + self.num_retries;

        loop {
            self.wait_rate_limit(org, resources, &send_batch).await?;
            let (req_builder, redacted_url, request_id) = self
                .build_request(&send_batch, batch_as_multipart, org)
                .await?;

            match req_builder.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let code = status.as_u16();
                    let status_line = go_status(&resp);
                    let headers = resp.headers().clone();

                    let (succeeded, retry_strat) = self.check_status(code);
                    let body_bytes = read_capped_body(resp, self.max_response_bytes).await?;

                    if succeeded {
                        if let Some(id) = request_id.filter(|_| !self.named.swap(true, Ordering::Relaxed)) {
                            tracing::info!(request_id = %id, url = %redacted_url, status = code, "http_client: jc's listener answered; each call of this stream is X-Request-Id {}-<n>", self.run);
                        }
                        return Ok(self.response_to_batch(code, &headers, body_bytes));
                    }

                    let status_err = UnexpectedHttpStatusError {
                        code,
                        status: status_line,
                        body: String::from_utf8_lossy(&body_bytes).into_owned(),
                        redacted_url,
                    };

                    attempt += 1;
                    if retry_strat == RetryStrategy::NoRetry || attempt >= max_attempts {
                        return Err(CoreError::other(status_err.to_string()));
                    }

                    let sleep_dur = match retry_strat {
                        RetryStrategy::Linear => self.retry_period,
                        RetryStrategy::Exponential => {
                            let mult = 2u32.saturating_pow((attempt - 1) as u32);
                            self.retry_period.saturating_mul(mult).min(self.max_backoff)
                        }
                        RetryStrategy::NoRetry => unreachable!(),
                    };
                    tokio::time::sleep(sleep_dur).await;
                }
                Err(err) => {
                    attempt += 1;
                    let safe_err = redact_url_str(&error_chain(&err));
                    if attempt >= max_attempts {
                        return Err(CoreError::other(format!("{redacted_url}: {safe_err}")));
                    }
                    tokio::time::sleep(self.retry_period).await;
                }
            }
        }
    }

    fn response_to_batch(
        &self,
        code: u16,
        headers: &reqwest::header::HeaderMap,
        body: Bytes,
    ) -> Batch {
        let mut part = Part::new(body);
        part.meta_set("http_status_code", Value::Int(code as i64));
        if !self.meta_extract.is_empty() {
            for (k, v) in headers {
                let lower = k.as_str().to_ascii_lowercase();
                if self.meta_extract.matches(&lower) {
                    if let Ok(s) = v.to_str() {
                        part.meta_set_str(lower, s);
                    }
                }
            }
        }
        vec![part]
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RetryStrategy {
    NoRetry,
    Linear,
    Exponential,
}

async fn read_capped_body(resp: reqwest::Response, max_bytes: usize) -> Result<Bytes, CoreError> {
    if let Some(cl) = resp.content_length() {
        if cl as usize > max_bytes {
            return Err(CoreError::other(format!(
                "HTTP response Content-Length ({cl} bytes) exceeds maximum allowed ({max_bytes} bytes)"
            )));
        }
    }
    let mut stream = resp.bytes_stream();
    let mut buf = Vec::new();
    while let Some(chunk_res) = stream.next().await {
        let chunk = chunk_res.map_err(|e| CoreError::other(redact_url_str(&e.to_string())))?;
        if buf.len().saturating_add(chunk.len()) > max_bytes {
            return Err(CoreError::other(format!(
                "HTTP response body exceeds maximum allowed size ({max_bytes} bytes)"
            )));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(buf))
}

/// Validates that a config URL starts with `http://` or `https://` (or an interpolation `${...}`).
pub fn validate_url_scheme_prefix(raw_url: &str) -> Result<(), CoreError> {
    let trimmed = raw_url.trim();
    if trimmed.starts_with("${") {
        return Ok(());
    }
    if let Some(scheme_idx) = trimmed.find("://") {
        let scheme = &trimmed[..scheme_idx].to_ascii_lowercase();
        if scheme != "http" && scheme != "https" {
            return Err(CoreError::config(format!(
                "refused URL scheme '{scheme}': only 'http' and 'https' are allowed"
            )));
        }
        return Ok(());
    }
    Err(CoreError::config(format!(
        "URL '{}' must use 'http://' or 'https://' scheme",
        redact_url_str(trimmed)
    )))
}

/// Validates a fully resolved URL at request time:
/// 1. Scheme must be `http` or `https`.
/// 2. Checks SSRF private/loopback/link-local addresses unless `org.get_quotas().allow_private_ips` is true.
/// 3. Checks `org.check_http_host(host)` (allow-list); the caller then waits on
///    `org.admit_http_request()` (the RPS quota paces, it never fails).
pub fn validate_resolved_url(url_str: &str, org: &OrgContext) -> Result<Url, CoreError> {
    let parsed = Url::parse(url_str).map_err(|e| {
        CoreError::config(format!("invalid URL '{}': {e}", redact_url_str(url_str)))
    })?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(CoreError::AccessDenied {
            org: org.org_id.0.clone(),
            reason: format!("refused URL scheme '{scheme}': only 'http' and 'https' are allowed"),
        });
    }
    let host = parsed.host_str().ok_or_else(|| {
        CoreError::config(format!("URL '{}' has no host", redact_url_str(url_str)))
    })?;

    let allow_private =
        org.get_quotas().allow_private_ips || org.host_target().is_some_and(|t| t.admits(&parsed));
    if !allow_private && is_private_or_loopback_host(host) {
        return Err(CoreError::AccessDenied {
            org: org.org_id.0.clone(),
            reason: format!(
                "SSRF protection: outbound HTTP to private/loopback host '{host}' is refused"
            ),
        });
    }

    org.check_http_host(host)?;
    Ok(parsed)
}

fn build_http(timeout: Duration, guard: bool) -> Result<reqwest::Client, CoreError> {
    // Cap hops, keep http(s), and on the guarded client refuse a literal private host on every hop
    // (a DNS name is checked by the resolver, which reqwest uses for each hop's connection).
    let redirect_policy = RedirectPolicy::custom(move |attempt| {
        if attempt.previous().len() >= MAX_REDIRECTS {
            return attempt.error(format!("exceeded maximum redirects ({MAX_REDIRECTS})"));
        }
        let next_scheme = attempt.url().scheme().to_owned();
        if next_scheme != "http" && next_scheme != "https" {
            return attempt.error(format!("redirect to unsupported scheme '{next_scheme}'"));
        }
        if guard
            && attempt
                .url()
                .host_str()
                .is_some_and(is_private_or_loopback_host)
        {
            return attempt
                .error("SSRF protection: redirect to a private/loopback host is refused");
        }
        attempt.follow()
    });
    let connect_timeout = timeout
        .min(Duration::from_secs(5))
        .max(Duration::from_millis(200));
    let mut builder = reqwest::Client::builder()
        .redirect(redirect_policy)
        .connect_timeout(connect_timeout);
    if guard {
        builder = builder.dns_resolver(Arc::new(PublicOnlyResolver));
    }
    if !timeout.is_zero() {
        builder = builder.timeout(timeout);
    }
    builder
        .build()
        .map_err(|e| CoreError::config(format!("failed to build HTTP client: {e}")))
}

/// The client for [`ekiben_core::HostTarget`]: a literal address, so no resolver, and no redirect.
fn build_host_http(timeout: Duration) -> Result<reqwest::Client, CoreError> {
    let mut builder = reqwest::Client::builder()
        .redirect(RedirectPolicy::none())
        .connect_timeout(
            timeout
                .min(Duration::from_secs(5))
                .max(Duration::from_millis(200)),
        );
    if !timeout.is_zero() {
        builder = builder.timeout(timeout);
    }
    builder
        .build()
        .map_err(|e| CoreError::config(format!("failed to build HTTP client: {e}")))
}

/// Resolves through the system resolver and drops private addresses, so a public name that
/// resolves (or rebinds) to 127.0.0.1 or 169.254.169.254 never gets a connection.
struct PublicOnlyResolver;

impl reqwest::dns::Resolve for PublicOnlyResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let host = name.as_str().to_owned();
            let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .filter(|a| !is_private_or_loopback_ip(a.ip()))
                .collect();
            if addrs.is_empty() {
                return Err(format!(
                    "SSRF protection: '{host}' resolves only to private/loopback addresses"
                )
                .into());
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// reqwest's Display stops at "error sending request": append the causes (DNS refusal, TLS, refused).
fn error_chain(err: &dyn std::error::Error) -> String {
    let mut out = err.to_string();
    let mut src = err.source();
    while let Some(e) = src {
        out.push_str(": ");
        out.push_str(&e.to_string());
        src = e.source();
    }
    out
}

pub fn is_private_or_loopback_host(host: &str) -> bool {
    let clean = host.trim_matches(|c| c == '[' || c == ']');
    if clean.eq_ignore_ascii_case("localhost") || clean.ends_with(".localhost") {
        return true;
    }
    if let Ok(ip) = clean.parse::<IpAddr>() {
        return is_private_or_loopback_ip(ip);
    }
    false
}

pub fn is_private_or_loopback_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let octs = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || (octs[0] == 100 && (octs[1] & 0b1100_0000) == 64) // CGNAT 100.64.0.0/10
                || (octs[0] == 169 && octs[1] == 254) // AWS/cloud metadata 169.254.169.254
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_or_loopback_ip(IpAddr::V4(v4));
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // Unique local fc00::/7
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // Link-local fe80::/10
        }
    }
}

pub fn extract_status_code_from_err(err_msg: &str) -> Option<i64> {
    let marker = "unexpected response code (";
    let start = err_msg.find(marker)? + marker.len();
    let end = err_msg[start..].find(')')? + start;
    err_msg[start..end].parse::<i64>().ok()
}
