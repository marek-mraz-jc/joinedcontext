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
//!   [`TokenExchanger`], and only at the host's own gateway Endpoints
//!   ([`OutboundGuard::token_endpoint`]).

use crate::guard::{EndpointBase, GuardConfig, OperatorReach, OutboundGuard, OwnReach};
use crate::{
    Datasource, DsError, LokiDatasource, NgsiLdDatasource, ODataDatasource, PostgresDatasource,
    PrometheusDatasource, SecretResolver, TokenExchanger,
};
use daiya_model::{DatasourceRecord, OrgId};
use ipnet::IpNet;
use serde_json::Value;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgSslMode};
use std::collections::BTreeSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

/// Operator settings every built driver's guard shares.
#[derive(Debug, Clone)]
pub struct OutboundPolicy {
    /// Allow `http://` datasource URLs (in-cluster services usually speak plain http).
    pub allow_http: bool,
    /// Non-public networks the operator opens, each to one organization's datasources of the
    /// kinds it names ([`OpenedNetwork`]); no network is open to every organization.
    pub allowed_networks: Vec<OpenedNetwork>,
    pub timeout: Duration,
    pub max_response_bytes: usize,
    /// The host's own gateway Endpoints ([`GuardConfig::own_reach`]), for the `ngsild` and
    /// `odata` kinds only: the guard admits a request by its URL, whoever made the record.
    pub own_reach: Vec<OwnReach>,
    /// The host's gateway Endpoints at an address of their own ([`GuardConfig::endpoint_bases`]),
    /// for the `ngsild` and `odata` kinds only: with `own_reach`, the only URLs a caller's token
    /// is sent to.
    pub endpoint_bases: Vec<EndpointBase>,
    /// The one address the `prometheus` kind may reach over `http` on a non-public network, and
    /// only for datasources of this organization ([`GuardConfig::operator_reach`]).
    pub prometheus_reach: Option<(OrgId, OperatorReach)>,
}

impl Default for OutboundPolicy {
    fn default() -> Self {
        let guard = GuardConfig::default();
        Self {
            allow_http: false,
            allowed_networks: Vec::new(),
            timeout: guard.timeout,
            max_response_bytes: guard.max_response_bytes,
            own_reach: Vec::new(),
            endpoint_bases: Vec::new(),
            prometheus_reach: None,
        }
    }
}

/// One non-public network the operator opens (an in-cluster Prometheus or broker on the
/// cluster's service network), to the datasources of one organization and of the driver kinds
/// named ([`DRIVER_KINDS`]: a Grafana-kind record opens under its driver's kind, so
/// `grafana-postgresql-datasource` under `postgres`). The datasource's allow-list still applies
/// and the cloud metadata addresses stay blocked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedNetwork {
    pub network: IpNet,
    pub org_id: OrgId,
    pub kinds: Vec<String>,
}

impl OpenedNetwork {
    #[must_use]
    pub fn new(network: IpNet, org_id: OrgId, kinds: &[&str]) -> Self {
        Self {
            network,
            org_id,
            kinds: kinds.iter().map(|k| (*k).to_owned()).collect(),
        }
    }

    fn opens_to(&self, record: &DatasourceRecord) -> bool {
        self.org_id == record.org_id
            && driver_kind(&record.kind).is_some_and(|kind| self.kinds.iter().any(|k| k == kind))
    }
}

/// The driver kinds [`build_driver`] builds, the names an [`OpenedNetwork`] accepts.
pub const DRIVER_KINDS: [&str; 5] = ["prometheus", "ngsild", "odata", "postgres", "loki"];

/// The driver kind a record's `type` builds (`grafana-postgresql-datasource` builds `postgres`),
/// `None` for a kind no built-in driver serves.
#[must_use]
pub fn driver_kind(record_kind: &str) -> Option<&'static str> {
    match record_kind {
        "ngsild-grafana-datasource" => Some("ngsild"),
        "grafana-postgresql-datasource" => Some("postgres"),
        other => DRIVER_KINDS.into_iter().find(|k| *k == other),
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

/// The guard of one datasource: its allow-list (or its URL host) and the operator policy, with
/// only the networks opened to the record's organization and kind. Only
/// an NGSI-LD or OData datasource gets the policy's [`OutboundPolicy::own_reach`] and [`OutboundPolicy::endpoint_bases`], only a
/// Prometheus datasource of the named organization its [`OutboundPolicy::prometheus_reach`].
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
    let endpoint_kind = matches!(
        record.kind.as_str(),
        "ngsild" | "ngsild-grafana-datasource" | "odata"
    );
    Ok(OutboundGuard::new(GuardConfig {
        allow_hosts,
        max_response_bytes: policy.max_response_bytes,
        timeout: policy.timeout,
        allow_http: policy.allow_http,
        allow_loopback_for_tests: false,
        allowed_networks: policy
            .allowed_networks
            .iter()
            .filter(|opened| opened.opens_to(record))
            .map(|opened| opened.network)
            .collect(),
        own_reach: if endpoint_kind {
            policy.own_reach.clone()
        } else {
            Vec::new()
        },
        endpoint_bases: if endpoint_kind {
            policy.endpoint_bases.clone()
        } else {
            Vec::new()
        },
        operator_reach: policy
            .prometheus_reach
            .as_ref()
            .filter(|(org, _)| record.kind == "prometheus" && record.org_id == *org)
            .map(|(_, reach)| reach.clone()),
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
    let ssl_mode =
        ssl_mode_of(json_str(record, "sslmode"), &host).map_err(|why| config_error(record, why))?;

    // The guard decides the address and the pool dials that address in every mode: a name
    // handed to the driver would be resolved again at connect time, past the guard.
    let addrs = guard.resolve_checked(&host, port).await?;
    let Some(addr) = addrs.first() else {
        return Err(DsError::GuardRefused(format!("no address for '{host}'")));
    };
    let mut opts = pinned_options(*addr, ssl_mode)
        .database(database)
        .username(user)
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

/// Connect options that dial the guard's checked address, whatever the TLS mode.
fn pinned_options(addr: SocketAddr, ssl_mode: PgSslMode) -> PgConnectOptions {
    PgConnectOptions::new()
        .host(&addr.ip().to_string())
        .port(addr.port())
        .ssl_mode(ssl_mode)
}

/// The record's `jsonData.sslmode`; without one the server must present a certificate from a
/// trusted CA (`verify-ca`). The pool dials the guard's pinned address and sqlx checks the
/// certificate name against the dialled host, so `verify-full` holds only for a host given as
/// an IP address.
fn ssl_mode_of(sslmode: Option<&str>, host: &str) -> Result<PgSslMode, String> {
    Ok(match sslmode.unwrap_or("verify-ca") {
        "disable" => PgSslMode::Disable,
        "allow" => PgSslMode::Allow,
        "prefer" => PgSslMode::Prefer,
        "require" => PgSslMode::Require,
        "verify-ca" => PgSslMode::VerifyCa,
        "verify-full" if host.trim_matches(['[', ']']).parse::<IpAddr>().is_ok() => {
            PgSslMode::VerifyFull
        }
        "verify-full" => {
            return Err(format!(
                "sslmode 'verify-full' needs the host as an IP address (the connection is pinned to the checked address); use 'verify-ca' for '{host}'"
            ));
        }
        other => {
            return Err(format!(
                "sslmode '{other}' is not supported (verify-ca, verify-full, require, prefer, allow, disable)"
            ));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn postgres_defaults_to_verify_ca_and_verify_full_needs_an_address_host() {
        assert!(matches!(
            ssl_mode_of(None, "db.example").unwrap(),
            PgSslMode::VerifyCa
        ));
        assert!(ssl_mode_of(Some("verify-full"), "db.example").is_err());
        assert!(matches!(
            ssl_mode_of(Some("verify-full"), "[2001:db8::4]").unwrap(),
            PgSslMode::VerifyFull
        ));
        assert!(matches!(
            ssl_mode_of(Some("verify-full"), "198.51.100.4").unwrap(),
            PgSslMode::VerifyFull
        ));
        assert!(ssl_mode_of(Some("sometimes"), "db.example").is_err());
    }

    #[test]
    fn postgres_dials_the_checked_address_in_every_ssl_mode() {
        let checked: SocketAddr = "198.51.100.4:6432".parse().unwrap();
        for sslmode in ["disable", "allow", "prefer", "require", "verify-ca"] {
            let mode = ssl_mode_of(Some(sslmode), "db.example").unwrap();
            let opts = pinned_options(checked, mode);
            assert_eq!(opts.get_host(), "198.51.100.4", "{sslmode}");
            assert_eq!(opts.get_port(), 6432, "{sslmode}");
        }
        let v6: SocketAddr = "[2001:db8::4]:5432".parse().unwrap();
        let mode = ssl_mode_of(Some("verify-full"), "[2001:db8::4]").unwrap();
        assert_eq!(pinned_options(v6, mode).get_host(), "2001:db8::4");
    }

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
