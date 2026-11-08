//! Field parsing shared by the `ngsi_ld` components: Bento-style names and defaults, the
//! Endpoint URL rule, and the `oauth2`, `tls`, `headers` and retry blocks.

use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use ekiben_core::config::parse_duration_str;
use ekiben_core::{redact_url_str, CoreError, SecretString};
use reqwest::header::{HeaderName, HeaderValue};
use serde_json::{Map, Value as JsonValue};
use url::Url;

/// Antares refuses a batch of more than 1000 items (400): the default and the ceiling.
pub const MAX_BATCH_COUNT: usize = 1000;
/// Antares refuses a body over 4 MiB (413): the ceiling of `max_batch_bytes`.
pub const MAX_BATCH_BYTES: usize = 4 * 1024 * 1024;
/// The default of `max_batch_bytes`.
pub const DEFAULT_BATCH_BYTES: usize = 1024 * 1024;
/// The default cap on one answer of the broker (a page of 1000 entities fits well within it).
pub const DEFAULT_MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
/// The ceiling of `max_response_bytes`.
pub const MAX_RESPONSE_BYTES: usize = 256 * 1024 * 1024;
/// The ceiling of `max_in_flight` (Bento's `http_client` default).
pub const MAX_IN_FLIGHT: usize = 64;
/// The ceiling of `retries`.
pub const MAX_RETRIES: u64 = 1000;

/// The fields every component that calls a broker takes.
pub const CLIENT_FIELDS: &[&str] = &[
    "url",
    "timeout",
    "retries",
    "retry_period",
    "max_retry_backoff",
    "backoff_on",
    "drop_on",
    "successful_on",
    "rate_limit",
    "oauth2",
    "tls",
    "headers",
    "context",
    "tenant",
    "max_response_bytes",
];

/// The fields of one component config. A name the component does not know is refused, so a
/// typo never silently keeps a default.
pub struct Fields<'a> {
    obj: &'a Map<String, JsonValue>,
    component: &'static str,
}

impl<'a> Fields<'a> {
    pub fn new(
        conf: &'a JsonValue,
        component: &'static str,
        known: &[&str],
    ) -> Result<Self, CoreError> {
        let obj = conf.as_object().ok_or_else(|| {
            CoreError::config(format!("{component}: the config must be an object"))
        })?;
        if let Some(k) = obj.keys().find(|k| !known.contains(&k.as_str())) {
            return Err(CoreError::config(format!(
                "{component}: field {k} not recognised"
            )));
        }
        Ok(Self { obj, component })
    }

    pub fn component(&self) -> &'static str {
        self.component
    }

    pub fn err(&self, msg: impl std::fmt::Display) -> CoreError {
        CoreError::config(format!("{}: {msg}", self.component))
    }

    pub fn raw(&self, key: &str) -> Option<&'a JsonValue> {
        self.obj.get(key).filter(|v| !v.is_null())
    }

    pub fn str(&self, key: &str) -> Result<Option<&'a str>, CoreError> {
        match self.raw(key) {
            None => Ok(None),
            Some(JsonValue::String(s)) => Ok(Some(s.as_str())),
            Some(_) => Err(self.err(format!("field {key} must be a string"))),
        }
    }

    pub fn string_or(&self, key: &str, default: &str) -> Result<String, CoreError> {
        Ok(self.str(key)?.unwrap_or(default).to_string())
    }

    pub fn bool_or(&self, key: &str, default: bool) -> Result<bool, CoreError> {
        match self.raw(key) {
            None => Ok(default),
            Some(JsonValue::Bool(b)) => Ok(*b),
            Some(JsonValue::String(s)) if s == "true" || s == "false" => Ok(s == "true"),
            Some(_) => Err(self.err(format!("field {key} must be a bool"))),
        }
    }

    pub fn u64_or(&self, key: &str, default: u64) -> Result<u64, CoreError> {
        let bad = || self.err(format!("field {key} must be a non-negative integer"));
        match self.raw(key) {
            None => Ok(default),
            Some(JsonValue::Number(n)) => n.as_u64().ok_or_else(bad),
            Some(JsonValue::String(s)) => s.trim().parse().map_err(|_| bad()),
            Some(_) => Err(bad()),
        }
    }

    pub fn duration_or(&self, key: &str, default: Duration) -> Result<Duration, CoreError> {
        match self.str(key)? {
            None => Ok(default),
            Some(s) => parse_duration_str(s).map_err(|e| self.err(format!("field {key}: {e}"))),
        }
    }

    pub fn bytes_or(&self, key: &str, default: usize) -> Result<usize, CoreError> {
        let bad = || self.err(format!("field {key} must be a byte size such as 1MiB"));
        match self.raw(key) {
            None => Ok(default),
            Some(JsonValue::Number(n)) => n
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(bad),
            Some(JsonValue::String(s)) => parse_byte_size(s).ok_or_else(bad),
            Some(_) => Err(bad()),
        }
    }

    pub fn codes(&self, key: &str, default: &[u16]) -> Result<HashSet<u16>, CoreError> {
        let bad = || self.err(format!("field {key} must list HTTP status codes"));
        match self.raw(key) {
            None => Ok(default.iter().copied().collect()),
            Some(JsonValue::Array(items)) => items
                .iter()
                .map(|v| {
                    v.as_u64()
                        .and_then(|c| u16::try_from(c).ok())
                        .filter(|c| (100..=599).contains(c))
                        .ok_or_else(bad)
                })
                .collect(),
            Some(_) => Err(bad()),
        }
    }

    /// A list of strings, written as an array or as one comma separated string.
    pub fn strings(&self, key: &str) -> Result<Vec<String>, CoreError> {
        let bad = || self.err(format!("field {key} must list strings"));
        match self.raw(key) {
            None => Ok(Vec::new()),
            Some(JsonValue::String(s)) => Ok(s
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()),
            Some(JsonValue::Array(items)) => items
                .iter()
                .map(|v| v.as_str().map(String::from).ok_or_else(bad))
                .collect(),
            Some(_) => Err(bad()),
        }
    }

    pub fn object(&self, key: &str) -> Result<Option<&'a Map<String, JsonValue>>, CoreError> {
        match self.raw(key) {
            None => Ok(None),
            Some(JsonValue::Object(m)) => Ok(Some(m)),
            Some(_) => Err(self.err(format!("field {key} must be an object"))),
        }
    }
}

/// `1048576`, `512KiB`, `1MiB`, `4MB`, `100B` as a number of bytes.
pub fn parse_byte_size(text: &str) -> Option<usize> {
    let t = text.trim();
    let split = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
    let (num, unit) = t.split_at(split);
    let n: usize = num.parse().ok()?;
    let mult: usize = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "kb" => 1000,
        "kib" => 1024,
        "mb" => 1_000_000,
        "mib" => 1024 * 1024,
        _ => return None,
    };
    n.checked_mul(mult)
}

/// The broker base a component calls: an Endpoint URL (or an outside broker's) whose path ends
/// `/ngsi-ld/v1`. Never the broker root, never with credentials, a query or a fragment.
#[derive(Clone, Debug)]
pub struct BrokerUrl {
    base: String,
    /// The path names a gateway Endpoint (`/api/endpoint/`): the gateway owns tenant and context.
    pub platform: bool,
    scheme: String,
    host: String,
    port: Option<u16>,
    path: String,
}

impl BrokerUrl {
    pub fn parse(component: &'static str, raw: &str) -> Result<Self, CoreError> {
        let shown = redact_url_str(raw.trim());
        let bad = |why: &str| CoreError::config(format!("{component}: url {shown:?} {why}"));
        let trimmed = raw.trim().trim_end_matches('/');
        let url = Url::parse(trimmed).map_err(|e| bad(&format!("is not a URL: {e}")))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(bad("must use http or https"));
        }
        let host = url
            .host_str()
            .ok_or_else(|| bad("has no host"))?
            .to_ascii_lowercase();
        if !url.username().is_empty() || url.password().is_some() {
            return Err(bad(
                "must not carry credentials: no static API keys, the token comes from oauth2",
            ));
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err(bad(
                "must be the Endpoint base, without a query or fragment",
            ));
        }
        let path = url.path().trim_end_matches('/').to_string();
        if !path.ends_with("/ngsi-ld/v1") {
            return Err(bad(
                "must be the Endpoint base ending /ngsi-ld/v1 (never the broker root)",
            ));
        }
        Ok(Self {
            base: url.as_str().trim_end_matches('/').to_string(),
            platform: path.contains("/api/endpoint/"),
            scheme: url.scheme().to_string(),
            host,
            port: url.port_or_known_default(),
            path,
        })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// `rest` below the base: `entityOperations/upsert` becomes `<base>/entityOperations/upsert`.
    pub fn join(&self, rest: &str) -> String {
        format!("{}/{}", self.base, rest.trim_start_matches('/'))
    }

    /// Whether `url` stays on this broker: same scheme, host and port, no credentials, and a
    /// path under the base (a paging link the broker answered must not lead elsewhere).
    pub fn contains(&self, url: &Url) -> bool {
        url.scheme() == self.scheme
            && url.host_str().map(str::to_ascii_lowercase).as_deref() == Some(self.host.as_str())
            && url.port_or_known_default() == self.port
            && url.username().is_empty()
            && url.password().is_none()
            && (url.path() == self.path || url.path().starts_with(&format!("{}/", self.path)))
    }
}

/// Headers a component sets itself, or the gateway decides: a config may not write them.
const OWNED_HEADERS: &[&str] = &[
    "cookie",
    "link",
    "ngsild-tenant",
    "content-type",
    "content-length",
    "transfer-encoding",
    "host",
    "accept",
];

/// The `headers` field: extra headers, never `Authorization` (no static API keys) nor one the
/// component or the gateway owns.
pub fn parse_headers(f: &Fields<'_>) -> Result<Vec<(HeaderName, HeaderValue)>, CoreError> {
    let Some(map) = f.object("headers")? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(map.len());
    for (k, v) in map {
        let lower = k.to_ascii_lowercase();
        if lower == "authorization" || lower == "proxy-authorization" {
            return Err(f.err(
                "headers may not carry Authorization: no static API keys, the token comes from oauth2",
            ));
        }
        if OWNED_HEADERS.contains(&lower.as_str()) || lower.starts_with("x-jc-") {
            return Err(f.err(format!(
                "headers may not set {k}: the component or the gateway owns it (Link and NGSILD-Tenant come from context and tenant)"
            )));
        }
        let Some(s) = v.as_str() else {
            return Err(f.err(format!("header {k} must be a string")));
        };
        let name = HeaderName::from_bytes(k.as_bytes())
            .map_err(|_| f.err(format!("header name {k:?} is not valid")))?;
        let value = HeaderValue::from_str(s)
            .map_err(|_| f.err(format!("header {k} has a value HTTP does not allow")))?;
        out.push((name, value));
    }
    Ok(out)
}

/// The `oauth2` block, as ekiben's http client takes it. No `client_secret` is the pipeline
/// identity's token sidecar (`{project}/{pipeline}` as the client id, no secret).
#[derive(Clone)]
pub struct OAuth2Settings {
    pub client_key: String,
    pub client_secret: SecretString,
    pub token_url: String,
    pub scopes: Vec<String>,
    pub endpoint_params: BTreeMap<String, Vec<String>>,
}

impl std::fmt::Debug for OAuth2Settings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuth2Settings")
            .field("client_key", &self.client_key)
            .field("client_secret", &self.client_secret)
            .field("token_url", &redact_url_str(&self.token_url))
            .field("scopes", &self.scopes)
            .finish_non_exhaustive()
    }
}

pub fn parse_oauth2(f: &Fields<'_>) -> Result<Option<OAuth2Settings>, CoreError> {
    let Some(m) = f.object("oauth2")? else {
        return Ok(None);
    };
    let wrapped = JsonValue::Object(m.clone());
    let o = Fields::new(
        &wrapped,
        f.component(),
        &[
            "enabled",
            "client_key",
            "client_secret",
            "token_url",
            "scopes",
            "endpoint_params",
        ],
    )?;
    if !o.bool_or("enabled", false)? {
        return Ok(None);
    }
    let token_url = o.string_or("token_url", "")?.trim().to_string();
    let shown = redact_url_str(&token_url);
    let parsed = Url::parse(&token_url)
        .map_err(|_| f.err(format!("oauth2.token_url {shown:?} is not a URL")))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(f.err(format!(
            "oauth2.token_url {shown:?} must be an http or https URL with a host"
        )));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(f.err("oauth2.token_url must not carry credentials"));
    }
    let client_key = o.string_or("client_key", "")?;
    if client_key.is_empty() {
        return Err(f.err("oauth2.client_key is required when oauth2 is enabled"));
    }
    let client_secret = SecretString::new(o.string_or("client_secret", "")?);
    let scopes = o.strings("scopes")?;
    let mut endpoint_params = BTreeMap::new();
    if let Some(params) = o.object("endpoint_params")? {
        for (k, v) in params {
            let values = match v {
                JsonValue::String(s) => vec![s.clone()],
                JsonValue::Array(items) => items
                    .iter()
                    .map(|i| i.as_str().map(String::from))
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(|| {
                        f.err(format!("oauth2.endpoint_params.{k} must list strings"))
                    })?,
                _ => {
                    return Err(f.err(format!(
                        "oauth2.endpoint_params.{k} must be a string or a list of strings"
                    )))
                }
            };
            endpoint_params.insert(k.clone(), values);
        }
    }
    Ok(Some(OAuth2Settings {
        client_key,
        client_secret,
        token_url,
        scopes,
        endpoint_params,
    }))
}

/// The `tls` block: verification is always on, with the system roots.
pub fn check_tls(f: &Fields<'_>) -> Result<(), CoreError> {
    let Some(m) = f.object("tls")? else {
        return Ok(());
    };
    let wrapped = JsonValue::Object(m.clone());
    let t = Fields::new(
        &wrapped,
        f.component(),
        &[
            "enabled",
            "skip_cert_verify",
            "enable_renegotiation",
            "root_cas",
            "root_cas_file",
            "client_certs",
        ],
    )?;
    if t.bool_or("skip_cert_verify", false)? {
        return Err(f.err("tls.skip_cert_verify is refused: certificate verification is always on"));
    }
    if t.bool_or("enable_renegotiation", false)? {
        return Err(f.err("tls.enable_renegotiation is not supported"));
    }
    let named = |k: &str| t.str(k).map(|s| s.is_some_and(|s| !s.is_empty()));
    let certs = t
        .raw("client_certs")
        .and_then(JsonValue::as_array)
        .is_some_and(|a| !a.is_empty());
    if named("root_cas")? || named("root_cas_file")? || certs {
        return Err(f.err(
            "tls.root_cas, tls.root_cas_file and tls.client_certs are not supported by ngsi_ld in this build: certificates verify against the system roots",
        ));
    }
    Ok(())
}

/// Bento's retry fields (`internal/httpclient/config.go`) with their defaults.
#[derive(Clone, Debug)]
pub struct RetrySettings {
    pub retries: usize,
    pub retry_period: Duration,
    pub max_backoff: Duration,
    pub backoff_on: HashSet<u16>,
    pub drop_on: HashSet<u16>,
    pub successful_on: HashSet<u16>,
}

/// Everything the broker client needs besides the URL it calls.
#[derive(Clone, Debug)]
pub struct ClientSettings {
    pub timeout: Duration,
    pub retry: RetrySettings,
    pub oauth2: Option<OAuth2Settings>,
    pub headers: Vec<(HeaderName, HeaderValue)>,
    pub rate_limit: Option<String>,
    pub max_response_bytes: usize,
    /// A Link `@context` URL, for brokers outside the platform only.
    pub context: Option<String>,
    /// An `NGSILD-Tenant` value, for brokers outside the platform only.
    pub tenant: Option<HeaderValue>,
}

pub fn parse_client(
    f: &Fields<'_>,
    url: &BrokerUrl,
    default_timeout: Duration,
    default_drop_on: &[u16],
) -> Result<ClientSettings, CoreError> {
    check_tls(f)?;
    let retries = f.u64_or("retries", 3)?;
    if retries > MAX_RETRIES {
        return Err(f.err(format!("retries may be at most {MAX_RETRIES}")));
    }
    let retry = RetrySettings {
        retries: usize::try_from(retries).unwrap_or(usize::MAX),
        retry_period: f.duration_or("retry_period", Duration::from_secs(1))?,
        max_backoff: f.duration_or("max_retry_backoff", Duration::from_secs(300))?,
        backoff_on: f.codes("backoff_on", &[429])?,
        drop_on: f.codes("drop_on", default_drop_on)?,
        successful_on: f.codes("successful_on", &[])?,
    };
    let context = match f.str("context")?.map(str::trim) {
        None | Some("") | Some("none") => None,
        Some(link) => {
            let parsed = Url::parse(link)
                .map_err(|_| f.err(format!("context {link:?} must be none or a URL")))?;
            if !matches!(parsed.scheme(), "http" | "https") {
                return Err(f.err("context must be none or an http(s) URL"));
            }
            if link.contains(['<', '>', '"']) {
                return Err(f.err("context holds a character a Link header does not allow"));
            }
            Some(link.to_string())
        }
    };
    let tenant = match f.str("tenant")?.map(str::trim) {
        None | Some("") => None,
        Some(t) => Some(
            HeaderValue::from_str(t)
                .map_err(|_| f.err("tenant holds a character HTTP does not allow"))?,
        ),
    };
    if url.platform && (context.is_some() || tenant.is_some()) {
        return Err(f.err(
            "context and tenant are refused on a gateway Endpoint url (/api/endpoint/): the gateway sets NGSILD-Tenant itself and refuses a non-core @context",
        ));
    }
    let max_response_bytes = f.bytes_or("max_response_bytes", DEFAULT_MAX_RESPONSE_BYTES)?;
    if max_response_bytes == 0 || max_response_bytes > MAX_RESPONSE_BYTES {
        return Err(f.err(format!(
            "max_response_bytes must be between 1 and {MAX_RESPONSE_BYTES}"
        )));
    }
    Ok(ClientSettings {
        timeout: f.duration_or("timeout", default_timeout)?,
        retry,
        oauth2: parse_oauth2(f)?,
        headers: parse_headers(f)?,
        rate_limit: f
            .str("rate_limit")?
            .filter(|s| !s.is_empty())
            .map(String::from),
        max_response_bytes,
        context,
        tenant,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_sizes() {
        assert_eq!(parse_byte_size("1MiB"), Some(1024 * 1024));
        assert_eq!(parse_byte_size("4 MiB"), Some(4 * 1024 * 1024));
        assert_eq!(parse_byte_size("512KiB"), Some(512 * 1024));
        assert_eq!(parse_byte_size("1000"), Some(1000));
        assert_eq!(parse_byte_size("1kb"), Some(1000));
        assert_eq!(parse_byte_size("1GiB"), None);
        assert_eq!(parse_byte_size("MiB"), None);
        assert_eq!(parse_byte_size("99999999999999999999999MiB"), None);
    }

    #[test]
    fn broker_url_rule() {
        let ok = BrokerUrl::parse("t", "http://gw:8080/api/endpoint/x/ngsi-ld/v1/").expect("ok");
        assert!(ok.platform);
        assert_eq!(ok.base(), "http://gw:8080/api/endpoint/x/ngsi-ld/v1");
        assert_eq!(
            ok.join("entityOperations/upsert"),
            "http://gw:8080/api/endpoint/x/ngsi-ld/v1/entityOperations/upsert"
        );
        assert!(
            !BrokerUrl::parse("t", "https://b.example/ngsi-ld/v1")
                .expect("ok")
                .platform
        );
        for bad in [
            "http://gw:8080/",
            "http://gw:8080/api/endpoint/x",
            "ftp://gw/ngsi-ld/v1",
            "http://u:p@gw/ngsi-ld/v1",
            "http://gw/ngsi-ld/v1?options=update",
            "not a url",
        ] {
            assert!(BrokerUrl::parse("t", bad).is_err(), "{bad}");
        }
        let err = BrokerUrl::parse("t", "http://u:hunter2@gw/x")
            .unwrap_err()
            .to_string();
        assert!(!err.contains("hunter2"), "{err}");
    }

    #[test]
    fn broker_url_contains_only_its_own_paths() {
        let b = BrokerUrl::parse("t", "http://gw:8080/api/endpoint/x/ngsi-ld/v1").expect("ok");
        let yes = Url::parse("http://gw:8080/api/endpoint/x/ngsi-ld/v1/temporal/entities?a=1")
            .expect("url");
        assert!(b.contains(&yes));
        for no in [
            "http://evil:8080/api/endpoint/x/ngsi-ld/v1/temporal/entities",
            "https://gw:8080/api/endpoint/x/ngsi-ld/v1/temporal/entities",
            "http://gw:8081/api/endpoint/x/ngsi-ld/v1/temporal/entities",
            "http://gw:8080/api/endpoint/y/ngsi-ld/v1/temporal/entities",
            "http://gw:8080/api/endpoint/x/ngsi-ld/v1x",
        ] {
            assert!(!b.contains(&Url::parse(no).expect("url")), "{no}");
        }
    }

    #[test]
    fn oauth2_debug_redacts_the_secret() {
        let s = OAuth2Settings {
            client_key: "k".into(),
            client_secret: SecretString::new("hunter2"),
            token_url: "http://t/token".into(),
            scopes: vec![],
            endpoint_params: BTreeMap::new(),
        };
        let shown = format!("{s:?}");
        assert!(!shown.contains("hunter2"), "{shown}");
    }
}
