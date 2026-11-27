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
        // An http(s) URL always has a host: `Url::parse` refuses one without.
        let web = matches!(url.scheme(), "http" | "https");
        let Some(host) = url.host_str().filter(|_| web) else {
            return Err(bad("must use http or https"));
        };
        let host = host.to_ascii_lowercase();
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

    fn fields(conf: &JsonValue) -> Fields<'_> {
        Fields::new(conf, "t", &["a", "b", "headers", "oauth2", "tls"]).expect("fields")
    }

    /// The error text, or "" for an answer that should have been a refusal.
    fn err_of<T>(r: Result<T, CoreError>) -> String {
        r.err().map(|e| e.to_string()).unwrap_or_default()
    }

    #[test]
    fn fields_refuse_a_non_object_and_an_unknown_name() {
        let e = err_of(Fields::new(&serde_json::json!([1]), "t", &["a"]));
        assert!(e.contains("t: the config must be an object"), "{e}");
        let e = err_of(Fields::new(
            &serde_json::json!({"a": 1, "zz": 2}),
            "t",
            &["a"],
        ));
        assert!(e.contains("t: field zz not recognised"), "{e}");
        let conf = serde_json::json!({"a": null});
        let f = fields(&conf);
        assert_eq!(f.component(), "t");
        assert!(f.raw("a").is_none(), "null reads as absent");
    }

    #[test]
    fn fields_typed_readers_take_defaults_values_and_refuse_wrong_types() {
        let conf = serde_json::json!({"a": "x", "b": 3});
        let f = fields(&conf);
        assert_eq!(f.str("a").expect("str"), Some("x"));
        assert!(err_of(f.str("b")).contains("field b must be a string"));
        assert_eq!(f.string_or("headers", "d").expect("default"), "d");

        for (v, want) in [
            (serde_json::json!(true), true),
            (serde_json::json!("false"), false),
        ] {
            let conf = serde_json::json!({ "a": v });
            assert_eq!(fields(&conf).bool_or("a", !want).expect("bool"), want);
        }
        assert!(fields(&conf).bool_or("headers", true).expect("default"));
        let bad = serde_json::json!({"a": "yes"});
        assert!(err_of(fields(&bad).bool_or("a", false)).contains("field a must be a bool"));

        let conf = serde_json::json!({"a": 7, "b": " 9 "});
        let f = fields(&conf);
        assert_eq!(f.u64_or("a", 0).expect("n"), 7);
        assert_eq!(f.u64_or("b", 0).expect("s"), 9);
        assert_eq!(f.u64_or("headers", 4).expect("default"), 4);
        for bad in [
            serde_json::json!(-1),
            serde_json::json!("x"),
            serde_json::json!([]),
        ] {
            let conf = serde_json::json!({ "a": bad });
            let e = err_of(fields(&conf).u64_or("a", 0));
            assert!(e.contains("field a must be a non-negative integer"), "{e}");
        }

        let conf = serde_json::json!({"a": "2s", "b": "soon"});
        let f = fields(&conf);
        assert_eq!(
            f.duration_or("a", Duration::ZERO).expect("d"),
            Duration::from_secs(2)
        );
        assert_eq!(
            f.duration_or("headers", Duration::from_secs(5))
                .expect("default"),
            Duration::from_secs(5)
        );
        assert!(err_of(f.duration_or("b", Duration::ZERO)).contains("t: field b:"));
    }

    #[test]
    fn fields_bytes_codes_strings_and_objects() {
        let conf = serde_json::json!({"a": 10, "b": "2KiB"});
        let f = fields(&conf);
        assert_eq!(f.bytes_or("a", 0).expect("n"), 10);
        assert_eq!(f.bytes_or("b", 0).expect("s"), 2048);
        assert_eq!(f.bytes_or("headers", 3).expect("default"), 3);
        for bad in [
            serde_json::json!(-1),
            serde_json::json!("1GiB"),
            serde_json::json!(true),
        ] {
            let conf = serde_json::json!({ "a": bad });
            let e = err_of(fields(&conf).bytes_or("a", 0));
            assert!(
                e.contains("field a must be a byte size such as 1MiB"),
                "{e}"
            );
        }

        let conf = serde_json::json!({"a": [100, 599]});
        let got = fields(&conf).codes("a", &[]).expect("codes");
        assert_eq!(got, HashSet::from([100, 599]));
        assert_eq!(
            fields(&conf).codes("b", &[429]).expect("default"),
            HashSet::from([429])
        );
        for bad in [
            serde_json::json!([99]),
            serde_json::json!([600]),
            serde_json::json!([70000]),
            serde_json::json!(["x"]),
            serde_json::json!(429),
        ] {
            let conf = serde_json::json!({ "a": bad });
            let e = err_of(fields(&conf).codes("a", &[]));
            assert!(e.contains("field a must list HTTP status codes"), "{e}");
        }

        let conf = serde_json::json!({"a": " x, ,y ", "b": ["p", "q"]});
        let f = fields(&conf);
        assert_eq!(f.strings("a").expect("csv"), vec!["x", "y"]);
        assert_eq!(f.strings("b").expect("array"), vec!["p", "q"]);
        assert!(f.strings("headers").expect("absent").is_empty());
        for bad in [serde_json::json!([1]), serde_json::json!(1)] {
            let conf = serde_json::json!({ "a": bad });
            assert!(err_of(fields(&conf).strings("a")).contains("field a must list strings"));
        }

        let conf = serde_json::json!({"a": {"k": 1}, "b": 1});
        let f = fields(&conf);
        assert_eq!(f.object("a").expect("obj").map(Map::len), Some(1));
        assert!(f.object("headers").expect("absent").is_none());
        assert!(err_of(f.object("b")).contains("field b must be an object"));
    }

    #[test]
    fn byte_size_units() {
        assert_eq!(parse_byte_size("100B"), Some(100));
        assert_eq!(parse_byte_size("4MB"), Some(4_000_000));
        assert_eq!(parse_byte_size(" 2 kib "), Some(2048));
    }

    #[test]
    fn broker_url_refusals_name_the_rule() {
        let e = err_of(BrokerUrl::parse("t", "unix:/ngsi-ld/v1"));
        assert!(e.contains("must use http or https"), "{e}");
        let e = err_of(BrokerUrl::parse("t", "http://gw/ngsi-ld/v1#frag"));
        assert!(e.contains("without a query or fragment"), "{e}");
        let e = err_of(BrokerUrl::parse("t", "http://u@gw/ngsi-ld/v1"));
        assert!(e.contains("must not carry credentials"), "{e}");
        let e = err_of(BrokerUrl::parse("t", "http://gw/v1"));
        assert!(e.contains("ending /ngsi-ld/v1"), "{e}");
    }

    #[test]
    fn broker_url_contains_refuses_credentials_and_takes_the_base_itself() {
        let b = BrokerUrl::parse("t", "http://GW:8080/ngsi-ld/v1").expect("ok");
        assert!(b.contains(&Url::parse("http://gw:8080/ngsi-ld/v1").expect("url")));
        for no in [
            "http://u@gw:8080/ngsi-ld/v1/entities",
            "http://:p@gw:8080/ngsi-ld/v1/entities",
            "file:///ngsi-ld/v1/entities",
        ] {
            assert!(!b.contains(&Url::parse(no).expect("url")), "{no}");
        }
    }

    #[test]
    fn headers_pass_extra_and_refuse_owned_auth_and_bad_values() {
        let conf = serde_json::json!({"headers": {"X-Trace": "1"}});
        let got = parse_headers(&fields(&conf)).expect("headers");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0.as_str(), "x-trace");
        assert_eq!(got[0].1, "1");
        assert!(parse_headers(&fields(&serde_json::json!({})))
            .expect("none")
            .is_empty());
        for (headers, want) in [
            (
                serde_json::json!({"Authorization": "Bearer x"}),
                "may not carry Authorization",
            ),
            (
                serde_json::json!({"Proxy-Authorization": "x"}),
                "may not carry Authorization",
            ),
            (
                serde_json::json!({"NGSILD-Tenant": "x"}),
                "headers may not set NGSILD-Tenant",
            ),
            (
                serde_json::json!({"X-JC-Org": "x"}),
                "headers may not set X-JC-Org",
            ),
            (serde_json::json!({"X-A": 1}), "header X-A must be a string"),
            (serde_json::json!({"bad name": "x"}), "is not valid"),
            (
                serde_json::json!({"X-A": "a\nb"}),
                "has a value HTTP does not allow",
            ),
        ] {
            let conf = serde_json::json!({ "headers": headers });
            let e = err_of(parse_headers(&fields(&conf)));
            assert!(e.contains(want), "{e} / {want}");
            assert!(!e.contains("Bearer x"), "{e}");
        }
    }

    #[test]
    fn oauth2_block_parses_and_refuses() {
        let none = serde_json::json!({});
        assert!(parse_oauth2(&fields(&none)).expect("absent").is_none());
        let off = serde_json::json!({"oauth2": {"enabled": false, "client_key": "k"}});
        assert!(parse_oauth2(&fields(&off)).expect("off").is_none());
        let on = serde_json::json!({"oauth2": {
            "enabled": true, "client_key": "k", "client_secret": "s",
            "token_url": " https://kc/token ", "scopes": "a,b",
            "endpoint_params": {"audience": "gw", "resource": ["r1", "r2"]}
        }});
        let o = parse_oauth2(&fields(&on)).expect("on").expect("enabled");
        assert_eq!(o.client_key, "k");
        assert_eq!(o.token_url, "https://kc/token");
        assert_eq!(o.scopes, vec!["a", "b"]);
        assert_eq!(o.endpoint_params["audience"], vec!["gw"]);
        assert_eq!(o.endpoint_params["resource"], vec!["r1", "r2"]);
        for (block, want) in [
            (
                serde_json::json!({"enabled": true, "zz": 1}),
                "field zz not recognised",
            ),
            (
                serde_json::json!({"enabled": true, "token_url": "no"}),
                "is not a URL",
            ),
            (
                serde_json::json!({"enabled": true, "token_url": "ftp://kc/t"}),
                "must be an http or https URL with a host",
            ),
            (
                serde_json::json!({"enabled": true, "token_url": "http://u:pw@kc/t"}),
                "oauth2.token_url must not carry credentials",
            ),
            (
                serde_json::json!({"enabled": true, "token_url": "http://u@kc/t"}),
                "oauth2.token_url must not carry credentials",
            ),
            (
                serde_json::json!({"enabled": true, "token_url": "http://kc/t"}),
                "oauth2.client_key is required",
            ),
            (
                serde_json::json!({"enabled": true, "token_url": "http://kc/t", "client_key": "k",
                    "endpoint_params": {"a": [1]}}),
                "oauth2.endpoint_params.a must list strings",
            ),
            (
                serde_json::json!({"enabled": true, "token_url": "http://kc/t", "client_key": "k",
                    "endpoint_params": {"a": 1}}),
                "oauth2.endpoint_params.a must be a string or a list of strings",
            ),
        ] {
            let conf = serde_json::json!({ "oauth2": block });
            let e = err_of(parse_oauth2(&fields(&conf)));
            assert!(e.contains(want), "{e} / {want}");
            assert!(!e.contains("pw@"), "{e}");
        }
    }

    #[test]
    fn tls_block_keeps_verification_on() {
        assert!(check_tls(&fields(&serde_json::json!({}))).is_ok());
        let ok = serde_json::json!({"tls": {"enabled": true, "root_cas": "", "client_certs": []}});
        assert!(check_tls(&fields(&ok)).is_ok());
        for (block, want) in [
            (serde_json::json!({"zz": 1}), "field zz not recognised"),
            (
                serde_json::json!({"skip_cert_verify": true}),
                "skip_cert_verify is refused",
            ),
            (
                serde_json::json!({"enable_renegotiation": true}),
                "not supported",
            ),
            (
                serde_json::json!({"root_cas": "PEM"}),
                "certificates verify against the system roots",
            ),
            (
                serde_json::json!({"root_cas_file": "/ca"}),
                "certificates verify against the system roots",
            ),
            (
                serde_json::json!({"client_certs": [{}]}),
                "certificates verify against the system roots",
            ),
        ] {
            let conf = serde_json::json!({ "tls": block });
            let e = err_of(check_tls(&fields(&conf)));
            assert!(e.contains(want), "{e} / {want}");
        }
    }

    fn client(conf: JsonValue, url: &str) -> Result<ClientSettings, CoreError> {
        let f = Fields::new(&conf, "t", CLIENT_FIELDS)?;
        let url = BrokerUrl::parse("t", url)?;
        parse_client(&f, &url, Duration::from_secs(7), &[400])
    }

    const OUTSIDE: &str = "https://b.example/ngsi-ld/v1";
    const PLATFORM: &str = "http://gw/api/endpoint/x/ngsi-ld/v1";

    #[test]
    fn client_settings_defaults_and_values() {
        let c = client(serde_json::json!({}), OUTSIDE).expect("defaults");
        assert_eq!(c.timeout, Duration::from_secs(7));
        assert_eq!(c.retry.retries, 3);
        assert_eq!(c.retry.retry_period, Duration::from_secs(1));
        assert_eq!(c.retry.max_backoff, Duration::from_secs(300));
        assert_eq!(c.retry.backoff_on, HashSet::from([429]));
        assert_eq!(c.retry.drop_on, HashSet::from([400]));
        assert!(c.retry.successful_on.is_empty());
        assert_eq!(c.max_response_bytes, DEFAULT_MAX_RESPONSE_BYTES);
        assert_eq!(DEFAULT_MAX_RESPONSE_BYTES, 67_108_864, "64 MiB");
        assert!(c.oauth2.is_none() && c.headers.is_empty() && c.rate_limit.is_none());
        assert!(c.context.is_none() && c.tenant.is_none());

        let c = client(
            serde_json::json!({"retries": 1000, "timeout": "3s", "rate_limit": "rl",
                "context": " https://ctx.example/c.jsonld ", "tenant": " t1 ",
                "max_response_bytes": "1KiB", "successful_on": [409]}),
            OUTSIDE,
        )
        .expect("values");
        assert_eq!(c.retry.retries, 1000);
        assert_eq!(c.timeout, Duration::from_secs(3));
        assert_eq!(c.rate_limit.as_deref(), Some("rl"));
        assert_eq!(c.context.as_deref(), Some("https://ctx.example/c.jsonld"));
        assert_eq!(c.tenant.as_ref().map(|t| t.to_str().ok()), Some(Some("t1")));
        assert_eq!(c.max_response_bytes, 1024);
        assert_eq!(c.retry.successful_on, HashSet::from([409]));

        for blank in [
            serde_json::json!({"context": "none", "tenant": "", "rate_limit": ""}),
            serde_json::json!({"context": " "}),
        ] {
            let c = client(blank, PLATFORM).expect("blank is absent");
            assert!(c.context.is_none() && c.tenant.is_none() && c.rate_limit.is_none());
        }
        assert_eq!(
            client(
                serde_json::json!({"max_response_bytes": MAX_RESPONSE_BYTES}),
                OUTSIDE
            )
            .expect("ceiling")
            .max_response_bytes,
            MAX_RESPONSE_BYTES
        );
    }

    #[test]
    fn client_settings_refusals() {
        for (conf, url, want) in [
            (
                serde_json::json!({"tls": {"skip_cert_verify": true}}),
                OUTSIDE,
                "skip_cert_verify",
            ),
            (
                serde_json::json!({"retries": 1001}),
                OUTSIDE,
                "retries may be at most 1000",
            ),
            (
                serde_json::json!({"context": "rel/path"}),
                OUTSIDE,
                "must be none or a URL",
            ),
            (
                serde_json::json!({"context": "ftp://c/x"}),
                OUTSIDE,
                "none or an http(s) URL",
            ),
            (
                serde_json::json!({"context": "http://c/\"x"}),
                OUTSIDE,
                "a Link header does not allow",
            ),
            (
                serde_json::json!({"tenant": "a\u{7f}b"}),
                OUTSIDE,
                "tenant holds a character",
            ),
            (
                serde_json::json!({"context": "http://c/x"}),
                PLATFORM,
                "refused on a gateway Endpoint",
            ),
            (
                serde_json::json!({"tenant": "t1"}),
                PLATFORM,
                "refused on a gateway Endpoint",
            ),
            (
                serde_json::json!({"max_response_bytes": 0}),
                OUTSIDE,
                "between 1 and",
            ),
            (
                serde_json::json!({"max_response_bytes": MAX_RESPONSE_BYTES + 1}),
                OUTSIDE,
                "between 1 and",
            ),
            (
                serde_json::json!({"timeout": "never"}),
                OUTSIDE,
                "field timeout",
            ),
            (
                serde_json::json!({"oauth2": {"enabled": true}}),
                OUTSIDE,
                "oauth2.token_url",
            ),
            (
                serde_json::json!({"headers": {"Cookie": "c"}}),
                OUTSIDE,
                "headers may not set",
            ),
            (
                serde_json::json!({"rate_limit": 1}),
                OUTSIDE,
                "field rate_limit must be a string",
            ),
        ] {
            let e = err_of(client(conf, url));
            assert!(e.contains(want), "{e} / {want}");
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
        assert!(
            shown.starts_with("OAuth2Settings { client_key: \"k\"")
                && shown.contains("http://t/token"),
            "{shown}"
        );
    }
}
