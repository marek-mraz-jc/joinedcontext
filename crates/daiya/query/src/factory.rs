#![forbid(unsafe_code)]
//! Driver factory (R3): builds the [`Datasource`] driver of a stored [`DatasourceRecord`], so a
//! datasource created through the API, the Grafana compat routes or provisioning can be
//! queried by a running server.
//!
//! - Every driver gets its own [`OutboundGuard`]: the record's `allowHosts`, or the host of its
//!   URL when the list is empty, plus the operator's [`OutboundPolicy`] (http allowed or not,
//!   which non-public networks are open). The Postgres driver resolves its host through the
//!   same guard and connects to the checked address.
//! - Credentials come from the record's `secretRefs`, resolved through the host's
//!   [`SecretResolver`]; a caller's token reaches NGSI-LD and OData only through the
//!   [`TokenExchanger`].

use crate::guard::{GuardConfig, OutboundGuard};
use crate::{
    Datasource, DsError, LokiDatasource, NgsiLdDatasource, ODataDatasource, PostgresDatasource,
    PrometheusDatasource, SecretResolver, TokenExchanger,
};
use daiya_model::DatasourceRecord;
use ipnet::IpNet;
use serde_json::Value;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgSslMode};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

/// Operator settings every built driver's guard shares.
#[derive(Debug, Clone)]
pub struct OutboundPolicy {
    /// Allow `http://` datasource URLs (in-cluster services usually speak plain http).
    pub allow_http: bool,
    /// Non-public networks the operator opens (for example the cluster's service network).
    pub allowed_networks: Vec<IpNet>,
    pub timeout: Duration,
    pub max_response_bytes: usize,
}

impl Default for OutboundPolicy {
    fn default() -> Self {
        let guard = GuardConfig::default();
        Self {
            allow_http: false,
            allowed_networks: Vec::new(),
            timeout: guard.timeout,
            max_response_bytes: guard.max_response_bytes,
        }
    }
}

/// What the factory needs from the host.
#[derive(Clone)]
pub struct DriverDeps {
    pub secrets: Arc<dyn SecretResolver>,
    pub token_exchanger: Arc<dyn TokenExchanger>,
    pub policy: OutboundPolicy,
}

fn config_error(record: &DatasourceRecord, reason: impl Into<String>) -> DsError {
    DsError::InvalidQuery {
        ref_id: record.uid.clone(),
        reason: format!("datasource configuration: {}", reason.into()),
    }
}

/// Host and port of a datasource URL: a full URL (`https://prom:9090/x`), a Postgres URL
/// (`postgres://db:5432/x`) or Grafana's Postgres form `host[:port]`.
fn host_and_port(record: &DatasourceRecord, default_port: u16) -> Result<(String, u16), DsError> {
    let raw = record.url.trim();
    if raw.is_empty() {
        return Err(config_error(record, "url is empty"));
    }
    let with_scheme = if raw.contains("://") {
        raw.to_owned()
    } else {
        format!("tcp://{raw}")
    };
    let parsed = url::Url::parse(&with_scheme)
        .map_err(|_| config_error(record, format!("url '{raw}' is not valid")))?;
    let host = parsed
        .host_str()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| config_error(record, "url has no host"))?
        .to_ascii_lowercase();
    let port = parsed.port_or_known_default().unwrap_or(default_port);
    Ok((host, port))
}

/// The guard of one datasource: its allow-list (or its URL host) and the operator policy.
pub fn guard_for(
    record: &DatasourceRecord,
    policy: &OutboundPolicy,
    default_port: u16,
) -> Result<OutboundGuard, DsError> {
    let allow_hosts = if record.allow_hosts.iter().any(|h| !h.trim().is_empty()) {
        record.allow_hosts.clone()
    } else {
        vec![host_and_port(record, default_port)?.0]
    };
    Ok(OutboundGuard::new(GuardConfig {
        allow_hosts,
        max_response_bytes: policy.max_response_bytes,
        timeout: policy.timeout,
        allow_http: policy.allow_http,
        allow_loopback_for_tests: false,
        allowed_networks: policy.allowed_networks.clone(),
    }))
}

fn json_str<'a>(record: &'a DatasourceRecord, key: &str) -> Option<&'a str> {
    record
        .json_data
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn json_bool(record: &DatasourceRecord, key: &str) -> bool {
    record
        .json_data
        .get(key)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Parses Grafana's `timeInterval` (`15s`, `1m`, `500ms`, `2h`) into milliseconds.
fn parse_interval_ms(raw: &str) -> Option<u64> {
    let raw = raw.trim();
    let split = raw.find(|c: char| !c.is_ascii_digit())?;
    let (num, unit) = raw.split_at(split);
    let n: u64 = num.parse().ok()?;
    let factor = match unit {
        "ms" => 1,
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => return None,
    };
    n.checked_mul(factor).filter(|ms| *ms > 0)
}

/// Builds the driver for `record`. Unknown kinds and WASM plugins without a loaded component
/// answer [`DsError::NotImplemented`]; a bad configuration answers [`DsError::InvalidQuery`];
/// a host the guard refuses answers [`DsError::GuardRefused`].
pub async fn build_driver(
    record: &DatasourceRecord,
    deps: &DriverDeps,
) -> Result<Arc<dyn Datasource>, DsError> {
    match record.kind.as_str() {
        "prometheus" => {
            let guard = guard_for(record, &deps.policy, 443)?;
            let mut driver = PrometheusDatasource::new(
                record.uid.clone(),
                record.url.clone(),
                guard,
                deps.secrets.clone(),
            );
            if let Some(ms) = json_str(record, "timeInterval").and_then(parse_interval_ms) {
                driver.scrape_interval_ms = ms;
            }
            if let Some(secret_ref) = ["token", "bearerToken", "httpHeaderValue1"]
                .iter()
                .find_map(|k| record.secret_refs.get(*k))
            {
                driver = driver.with_secret_ref(secret_ref.clone());
            }
            Ok(Arc::new(driver))
        }
        "ngsild" | "ngsild-grafana-datasource" => {
            let guard = guard_for(record, &deps.policy, 443)?;
            let mut driver = NgsiLdDatasource::new(
                record.uid.clone(),
                record.url.clone(),
                json_bool(record, "oauthPassThru"),
                guard,
                deps.token_exchanger.clone(),
            );
            if let Some(size) = record.json_data.get("pageSize").and_then(Value::as_u64) {
                driver.page_size = u32::try_from(size.clamp(1, 1_000)).unwrap_or(1_000);
            }
            driver.context_url = json_str(record, "contextUrl")
                .or_else(|| json_str(record, "@context"))
                .map(str::to_owned);
            driver.tenant = json_str(record, "tenant").map(str::to_owned);
            Ok(Arc::new(driver))
        }
        "odata" => {
            let guard = guard_for(record, &deps.policy, 443)?;
            let audience = json_str(record, "audience").unwrap_or(record.uid.as_str());
            Ok(Arc::new(ODataDatasource::new(
                record.uid.clone(),
                record.url.clone(),
                audience.to_owned(),
                guard,
                deps.token_exchanger.clone(),
            )))
        }
        "postgres" | "grafana-postgresql-datasource" => build_postgres(record, deps).await,
        "loki" => Ok(Arc::new(LokiDatasource::new(record.uid.clone()))),
        other if other.starts_with("wasm:") => Err(DsError::NotImplemented(format!(
            "WASM datasource '{other}' needs a loaded component"
        ))),
        other => Err(DsError::NotImplemented(format!(
            "datasource type '{other}'"
        ))),
    }
}

async fn build_postgres(
    record: &DatasourceRecord,
    deps: &DriverDeps,
) -> Result<Arc<dyn Datasource>, DsError> {
    let (host, port) = host_and_port(record, 5432)?;
    let guard = guard_for(record, &deps.policy, 5432)?;
    let database = json_str(record, "database")
        .ok_or_else(|| config_error(record, "jsonData.database is required"))?;
    let user = json_str(record, "user")
        .ok_or_else(|| config_error(record, "jsonData.user is required"))?;
    let allowed_views: BTreeSet<String> = record
        .json_data
        .get("allowedViews")
        .and_then(Value::as_array)
        .map(|views| {
            views
                .iter()
                .filter_map(Value::as_str)
                .map(|v| v.trim().to_ascii_lowercase())
                .filter(|v| !v.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if allowed_views.is_empty() {
        return Err(config_error(
            record,
            "jsonData.allowedViews must name the views granted to the organization",
        ));
    }
    let ssl_mode = match json_str(record, "sslmode").unwrap_or("prefer") {
        "disable" => PgSslMode::Disable,
        "allow" => PgSslMode::Allow,
        "prefer" => PgSslMode::Prefer,
        "require" => PgSslMode::Require,
        other => {
            return Err(config_error(
                record,
                format!("sslmode '{other}' is not supported (disable, allow, prefer, require)"),
            ));
        }
    };

    // The guard decides the address; the pool connects to that address, never re-resolving.
    let addrs = guard.resolve_checked(&host, port).await?;
    let Some(addr) = addrs.first() else {
        return Err(DsError::GuardRefused(format!("no address for '{host}'")));
    };
    let mut opts = PgConnectOptions::new()
        .host(&addr.ip().to_string())
        .port(addr.port())
        .database(database)
        .username(user)
        .ssl_mode(ssl_mode)
        .application_name("daiya");
    if let Some(secret_ref) = record.secret_refs.get("password") {
        let password = deps.secrets.resolve_secret(record.org_id, secret_ref)?;
        opts = opts.password(password.as_str());
    }
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(deps.policy.timeout)
        .connect_lazy_with(opts);
    Ok(Arc::new(PostgresDatasource::new(
        record.uid.clone(),
        allowed_views,
        json_bool(record, "timescaledb"),
        Some(pool),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intervals_parse_like_grafana_time_interval() {
        assert_eq!(parse_interval_ms("15s"), Some(15_000));
        assert_eq!(parse_interval_ms("500ms"), Some(500));
        assert_eq!(parse_interval_ms("2m"), Some(120_000));
        assert_eq!(parse_interval_ms("1h"), Some(3_600_000));
        assert_eq!(parse_interval_ms("1d"), Some(86_400_000));
        assert_eq!(parse_interval_ms("0s"), None);
        assert_eq!(parse_interval_ms("15"), None);
        assert_eq!(parse_interval_ms("s"), None);
        assert_eq!(parse_interval_ms("5w"), None);
    }
}
