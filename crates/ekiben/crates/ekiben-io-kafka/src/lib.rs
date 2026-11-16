#![forbid(unsafe_code)]
//! `ekiben-io-kafka`: Bento's `kafka_franz` input on a Kafka client of ekiben's own.
//!
//! Every seed broker and every broker a Metadata answer advertises is dialled through the
//! organization's outbound rule (ekiben-io-http `outbound::resolve_host_checked`), so a cluster
//! cannot point the runner at another address. TLS always verifies the broker's certificate
//! against the system roots plus `tls.root_cas`; `skip_cert_verify`, renegotiation, client
//! certificates and file paths are refused. SASL is PLAIN, SCRAM-SHA-256 or SCRAM-SHA-512 with
//! the password kept as a `SecretString`. A response is bounded by `broker_read_max_bytes`, a
//! batch inflates to at most 64 MiB.
//!
//! With `consumer_group` the input joins the group `<organization id>.<consumer_group>` (one
//! organization never joins another's group), takes partitions by the range assignor whatever
//! `group_balancers` names, and commits
//! the offset below its oldest unacknowledged batch every `commit_period` and at shutdown.
//! Without one it reads every partition of its topics and commits nothing. Each fetch hands the
//! pipeline one batch per partition; a nacked batch is fetched again (`auto_replay_nacks`), and a
//! partition with `checkpoint_limit` messages unacknowledged is not fetched until acks arrive.
//! The client speaks non-flexible API versions that Kafka 2.1 to 4.x and Redpanda serve
//! (Metadata v4, Fetch v4, ListOffsets v1, FindCoordinator v1, JoinGroup v2, SyncGroup v1,
//! Heartbeat v1, OffsetFetch v1, OffsetCommit v2, LeaveGroup v1, SaslHandshake v1,
//! SaslAuthenticate v0).

mod client;
mod consumer;
mod group;
mod records;
mod scram;
mod wire;

use std::time::Duration;

use ekiben_core::config::parse_duration_str;
use ekiben_core::redact::SecretString;
use ekiben_core::{lint_fields, ComponentConfig, CoreError, Registry};
use regex::Regex;
use serde_json::{Map, Value as JsonValue};

pub use consumer::KafkaInput;

const FIELDS: &[&str] = &[
    "seed_brokers",
    "topics",
    "regexp_topics",
    "consumer_group",
    "client_id",
    "rack_id",
    "checkpoint_limit",
    "auto_replay_nacks",
    "commit_period",
    "start_from_oldest",
    "reconnect_on_unknown_topic_or_partition",
    "auto_offset_reset",
    "group_balancers",
    "metadata_max_age",
    "fetch_max_bytes",
    "fetch_max_partition_bytes",
    "fetch_max_wait",
    "broker_read_max_bytes",
    "preferring_lag",
    "transaction_isolation_level",
    "tls",
    "sasl",
    "multi_header",
    "batching",
    "rate_limit",
    "extract_tracing_map",
    "new_root_span_with_link",
];
const TLS_FIELDS: &[&str] = &[
    "enabled",
    "skip_cert_verify",
    "enable_renegotiation",
    "root_cas",
    "root_cas_file",
    "client_certs",
];
const SASL_FIELDS: &[&str] = &[
    "mechanism",
    "username",
    "password",
    "token",
    "extensions",
    "aws",
];
/// The most `broker_read_max_bytes` may be set to.
const MAX_RESPONSE_CAP: usize = 256 << 20;

pub fn register_all(registry: &Registry) {
    registry.register_input("kafka_franz", |conf, ctx| {
        Ok(Box::new(KafkaInput::new(conf, ctx)?))
    });
    registry.register_lint("input", "kafka_franz", lint);
}

pub fn lint(conf: &JsonValue) -> Vec<String> {
    match parse(conf) {
        Ok(_) => Vec::new(),
        Err(e) => vec![e],
    }
}

pub(crate) enum Topics {
    Names(Vec<String>),
    Patterns(Vec<Regex>),
}

pub(crate) struct Settings {
    pub seeds: Vec<(String, u16)>,
    pub topics: Topics,
    pub consumer_group: Option<String>,
    pub client_id: String,
    pub checkpoint_limit: usize,
    pub auto_replay_nacks: bool,
    pub commit_period: Duration,
    pub oldest: bool,
    pub metadata_max_age: Duration,
    pub fetch_max_bytes: i32,
    pub fetch_max_partition_bytes: i32,
    pub fetch_max_wait: Duration,
    pub broker_read_max_bytes: usize,
    pub read_committed: bool,
    pub tls: Option<Vec<Vec<u8>>>,
    pub sasl: Option<client::Sasl>,
    pub multi_header: bool,
}

fn err(what: impl std::fmt::Display) -> String {
    format!("kafka_franz input: {what}")
}

fn str_field<'a>(obj: &'a Map<String, JsonValue>, k: &str) -> &'a str {
    obj.get(k).and_then(JsonValue::as_str).unwrap_or("")
}

fn bool_field(obj: &Map<String, JsonValue>, k: &str, default: bool) -> Result<bool, String> {
    match obj.get(k) {
        None | Some(JsonValue::Null) => Ok(default),
        Some(v) => v
            .as_bool()
            .ok_or_else(|| err(format!("{k} must be true or false"))),
    }
}

fn duration(obj: &Map<String, JsonValue>, k: &str, default: &str) -> Result<Duration, String> {
    let text = obj.get(k).and_then(JsonValue::as_str).unwrap_or(default);
    parse_duration_str(text).map_err(|_| err(format!("{k} is not a duration, e.g. {default}")))
}

fn byte_size(
    obj: &Map<String, JsonValue>,
    k: &str,
    default: usize,
    max: usize,
) -> Result<usize, String> {
    let n = match obj.get(k) {
        None | Some(JsonValue::Null) => default,
        Some(JsonValue::Number(n)) => n
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(usize::MAX),
        Some(JsonValue::String(s)) => {
            let t = s.trim();
            let split = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
            let (num, unit) = t.split_at(split);
            let mult: usize = match unit.trim().to_ascii_lowercase().as_str() {
                "" | "b" => 1,
                "kb" => 1000,
                "kib" => 1 << 10,
                "mb" => 1_000_000,
                "mib" => 1 << 20,
                "gb" => 1_000_000_000,
                "gib" => 1 << 30,
                _ => return Err(err(format!("{k} is not a size, e.g. 1MiB"))),
            };
            num.parse::<usize>()
                .ok()
                .and_then(|n| n.checked_mul(mult))
                .ok_or_else(|| err(format!("{k} is not a size, e.g. 1MiB")))?
        }
        Some(_) => return Err(err(format!("{k} is not a size, e.g. 1MiB"))),
    };
    if n == 0 || n > max {
        return Err(err(format!(
            "{k} must be between 1 byte and {} MiB",
            max >> 20
        )));
    }
    Ok(n)
}

/// `host:port`, an IPv6 address in brackets, or a host alone (port 9092).
fn broker_address(text: &str) -> Option<(String, u16)> {
    let t = text.trim();
    if t.is_empty() || t.contains('/') || t.contains('@') || t.chars().any(char::is_whitespace) {
        return None;
    }
    let (host, port) = if let Some(rest) = t.strip_prefix('[') {
        let (h, after) = rest.split_once(']')?;
        let port = match after.strip_prefix(':') {
            Some(p) => p.parse().ok()?,
            None if after.is_empty() => 9092,
            None => return None,
        };
        (format!("[{h}]"), port)
    } else {
        match t.rsplit_once(':') {
            Some((h, p)) if !h.contains(':') => (h.to_string(), p.parse().ok()?),
            Some(_) => return None,
            None => (t.to_string(), 9092),
        }
    };
    (!host.is_empty() && port != 0).then_some((host, port))
}

fn strings(obj: &Map<String, JsonValue>, k: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for (i, v) in obj
        .get(k)
        .and_then(JsonValue::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let s = v
            .as_str()
            .ok_or_else(|| err(format!("{k}[{i}] must be a string")))?;
        // Bento expands a comma-separated item.
        out.extend(
            s.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from),
        );
    }
    Ok(out)
}

/// A field Bento has and ekiben does not implement: refused unless empty.
fn refuse_set(obj: &Map<String, JsonValue>, k: &str, why: &str) -> Result<(), String> {
    let set = match obj.get(k) {
        None | Some(JsonValue::Null) => false,
        Some(JsonValue::String(s)) => !s.is_empty(),
        Some(JsonValue::Object(o)) => !o.is_empty(),
        Some(JsonValue::Array(a)) => !a.is_empty(),
        Some(JsonValue::Bool(b)) => *b,
        Some(JsonValue::Number(_)) => true,
    };
    if set {
        return Err(err(format!("{k} is not supported by ekiben: {why}")));
    }
    Ok(())
}

fn parse_tls(t: &Map<String, JsonValue>) -> Result<Option<Vec<Vec<u8>>>, String> {
    if let Some(k) = t.keys().find(|k| !TLS_FIELDS.contains(&k.as_str())) {
        return Err(err(format!("tls.{k} is not a field of tls")));
    }
    if bool_field(t, "skip_cert_verify", false)? {
        return Err(err("tls.skip_cert_verify: true is refused: ekiben always verifies the broker's certificate; give its CA in tls.root_cas"));
    }
    if bool_field(t, "enable_renegotiation", false)? {
        return Err(err(
            "tls.enable_renegotiation: true is refused: TLS renegotiation is not supported",
        ));
    }
    if !str_field(t, "root_cas_file").is_empty() {
        return Err(err(
            "tls.root_cas_file is not supported by ekiben; give the CA's PEM in tls.root_cas",
        ));
    }
    if t.get("client_certs")
        .and_then(JsonValue::as_array)
        .is_some_and(|c| !c.is_empty())
    {
        return Err(err("tls.client_certs is not supported by ekiben; authenticate with sasl and a password secret"));
    }
    if !bool_field(t, "enabled", false)? {
        return Ok(None);
    }
    let pem = str_field(t, "root_cas");
    if pem.is_empty() {
        return Ok(Some(Vec::new()));
    }
    ekiben_io_http::websocket::pem_certificates(pem)
        .map(Some)
        .map_err(|e| err(e.to_string().trim_start_matches("configuration error: ")))
}

fn parse_sasl(obj: &Map<String, JsonValue>) -> Result<Option<client::Sasl>, String> {
    let list = match obj.get("sasl") {
        None | Some(JsonValue::Null) => return Ok(None),
        Some(JsonValue::Array(a)) => a,
        Some(_) => return Err(err("sasl must be a list of mechanisms")),
    };
    let mut chosen = None;
    for (i, s) in list.iter().enumerate() {
        let s = s
            .as_object()
            .ok_or_else(|| err(format!("sasl[{i}] must be an object")))?;
        if let Some(k) = s.keys().find(|k| !SASL_FIELDS.contains(&k.as_str())) {
            return Err(err(format!("sasl[{i}].{k} is not a field of sasl")));
        }
        let mechanism = match str_field(s, "mechanism") {
            "" | "none" => continue,
            "PLAIN" => client::Mechanism::Plain,
            "SCRAM-SHA-256" => client::Mechanism::ScramSha256,
            "SCRAM-SHA-512" => client::Mechanism::ScramSha512,
            "OAUTHBEARER" | "AWS_MSK_IAM" => {
                return Err(err(format!(
                    "sasl[{i}].mechanism {} is not supported by ekiben; use SCRAM-SHA-512, SCRAM-SHA-256 or PLAIN over TLS",
                    str_field(s, "mechanism")
                )))
            }
            other => return Err(err(format!("sasl[{i}].mechanism {other} is not a SASL mechanism; use SCRAM-SHA-512, SCRAM-SHA-256 or PLAIN"))),
        };
        if chosen.is_some() {
            return Err(err(
                "sasl lists more than one mechanism; keep the one the cluster uses",
            ));
        }
        let username = str_field(s, "username");
        let password = str_field(s, "password");
        if username.is_empty() || password.is_empty() {
            return Err(err(format!(
                "sasl[{i}] needs username and password, e.g. password: ${{KAFKA_PASSWORD}} from a secret"
            )));
        }
        chosen = Some(client::Sasl {
            mechanism,
            username: username.to_string(),
            password: SecretString::new(password),
        });
    }
    Ok(chosen)
}

pub(crate) fn parse(conf: &JsonValue) -> Result<Settings, String> {
    let errs = lint_fields(conf, FIELDS, &["seed_brokers", "topics"]);
    if !errs.is_empty() {
        return Err(err(errs.join("; ")));
    }
    let obj = conf.as_object().ok_or_else(|| err("expected an object"))?;
    let mut seeds = Vec::new();
    for b in strings(obj, "seed_brokers")? {
        seeds.push(broker_address(&b).ok_or_else(|| {
            err(format!("seed_brokers: {b} is not a broker address; expected host:port, e.g. kafka.example.org:9092"))
        })?);
    }
    if seeds.is_empty() {
        return Err(err(
            "seed_brokers needs at least one broker, e.g. kafka.example.org:9092",
        ));
    }
    let names = strings(obj, "topics")?;
    if names.is_empty() {
        return Err(err(
            "topics needs at least one topic, e.g. vehicle-positions",
        ));
    }
    let topics = if bool_field(obj, "regexp_topics", false)? {
        let mut res = Vec::new();
        for t in &names {
            res.push(
                regex::RegexBuilder::new(&format!("^(?:{t})$"))
                    .size_limit(1 << 20)
                    .build()
                    .map_err(|_| {
                        err(format!(
                            "topics: {t} is not a regular expression, e.g. traffic-.*"
                        ))
                    })?,
            );
        }
        Topics::Patterns(res)
    } else {
        for t in &names {
            if t.contains(':') {
                return Err(err(format!(
                    "topics: {t} names explicit partitions, which ekiben does not support; name the topic and let the group assign partitions"
                )));
            }
            if t.len() > 249
                || !t
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
            {
                return Err(err(format!(
                    "topics: {t} is not a Kafka topic name (letters, digits, '.', '_', '-', at most 249)"
                )));
            }
        }
        Topics::Names(names)
    };
    let consumer_group = match str_field(obj, "consumer_group") {
        "" => None,
        g if g.len() > 200 || g.chars().any(char::is_control) => {
            return Err(err(
                "consumer_group must be a name of at most 200 characters, e.g. traffic-ingest",
            ))
        }
        g => Some(g.to_string()),
    };
    let client_id = match obj.get("client_id").and_then(JsonValue::as_str) {
        None => "bento".to_string(),
        Some(c) if c.len() > 200 || c.chars().any(char::is_control) => {
            return Err(err(
                "client_id must be a name of at most 200 characters, e.g. bento",
            ))
        }
        Some(c) => c.to_string(),
    };
    let checkpoint_limit = match obj.get("checkpoint_limit") {
        None | Some(JsonValue::Null) => 1024,
        Some(v) => v
            .as_u64()
            .filter(|n| (1..=1 << 20).contains(n))
            .ok_or_else(|| {
                err("checkpoint_limit must be a whole number from 1 to 1048576, e.g. 1024")
            })? as usize,
    };
    let oldest = match str_field(obj, "auto_offset_reset") {
        "" => bool_field(obj, "start_from_oldest", true)?,
        "earliest" => true,
        "latest" => false,
        other => {
            return Err(err(format!(
                "auto_offset_reset {other} is not supported; use earliest or latest"
            )))
        }
    };
    // Bento's balancer names are accepted (the form fills cooperative_sticky) and every ekiben
    // member assigns with range: members of one group must agree on the protocol, and range is
    // the one ekiben implements.
    for (i, b) in obj
        .get("group_balancers")
        .and_then(JsonValue::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        match b.as_str() {
            Some("cooperative_sticky" | "sticky" | "range" | "round_robin") => {}
            _ => {
                return Err(err(format!(
                    "group_balancers[{i}] is not a balancer; use range (ekiben assigns partitions with range)"
                )))
            }
        }
    }
    let read_committed = match str_field(obj, "transaction_isolation_level") {
        "" | "read_uncommitted" => false,
        "read_committed" => true,
        other => {
            return Err(err(format!(
                "transaction_isolation_level {other} is not one; use read_uncommitted or read_committed"
            )))
        }
    };
    refuse_set(
        obj,
        "rack_id",
        "it selects a follower replica (Fetch v11), which ekiben's client does not request",
    )?;
    refuse_set(
        obj,
        "preferring_lag",
        "ekiben does not reorder partitions by lag",
    )?;
    refuse_set(obj, "batching", "each fetch is one batch per partition; batch later with a broker input or an output's batching")?;
    refuse_set(obj, "rate_limit", "rate limits are not wired to this input")?;
    refuse_set(
        obj,
        "extract_tracing_map",
        "ekiben does not carry tracing spans",
    )?;
    refuse_set(
        obj,
        "new_root_span_with_link",
        "ekiben does not carry tracing spans",
    )?;
    let tls = match obj.get("tls") {
        None | Some(JsonValue::Null) => None,
        Some(JsonValue::Object(t)) => parse_tls(t)?,
        Some(_) => return Err(err("tls must be an object")),
    };
    let sasl = parse_sasl(obj)?;
    if matches!(&sasl, Some(s) if s.mechanism == client::Mechanism::Plain) && tls.is_none() {
        return Err(err(
            "sasl PLAIN sends the password in the clear; enable tls (tls.enabled: true) or use SCRAM-SHA-512",
        ));
    }
    let fetch_max_bytes = byte_size(obj, "fetch_max_bytes", 50 << 20, 128 << 20)?;
    let broker_read_max_bytes =
        byte_size(obj, "broker_read_max_bytes", 100 << 20, MAX_RESPONSE_CAP)?;
    if fetch_max_bytes > broker_read_max_bytes {
        return Err(err(
            "fetch_max_bytes must not be larger than broker_read_max_bytes",
        ));
    }
    Ok(Settings {
        seeds,
        topics,
        consumer_group,
        client_id,
        checkpoint_limit,
        auto_replay_nacks: bool_field(obj, "auto_replay_nacks", true)?,
        commit_period: duration(obj, "commit_period", "5s")?,
        oldest,
        metadata_max_age: duration(obj, "metadata_max_age", "5m")?,
        fetch_max_bytes: fetch_max_bytes as i32,
        fetch_max_partition_bytes: byte_size(obj, "fetch_max_partition_bytes", 1 << 20, 64 << 20)?
            as i32,
        fetch_max_wait: duration(obj, "fetch_max_wait", "5s")?,
        broker_read_max_bytes,
        read_committed,
        tls,
        sasl,
        multi_header: bool_field(obj, "multi_header", false)?,
    })
}

pub(crate) fn config_error(e: String) -> CoreError {
    CoreError::config(e)
}

pub(crate) fn settings(conf: &ComponentConfig) -> Result<Settings, CoreError> {
    parse(&conf.config).map_err(config_error)
}

#[cfg(test)]
mod live;
#[cfg(test)]
mod tests;
