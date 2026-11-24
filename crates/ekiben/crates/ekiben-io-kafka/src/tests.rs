use std::collections::HashMap;
use std::sync::Arc;

use ekiben_core::{Input, OrgId, OrgQuotas, ResourceManager, StreamContext, StreamMetrics};
use serde_json::json;

use super::*;

pub(crate) mod client;
pub(crate) mod fake;

fn refusal(v: JsonValue) -> String {
    match parse(&v) {
        Ok(_) => panic!("accepted {v}"),
        Err(e) => e,
    }
}

pub(crate) fn ctx(quotas: OrgQuotas) -> StreamContext {
    // every log line the input writes is formatted once, so its arguments are exercised too
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_test_writer()
        .try_init();
    let resources = ResourceManager::new();
    let org = resources.set_org_quotas(&OrgId::new("org-kafka"), quotas);
    StreamContext {
        stream_id: "kafka-test".into(),
        org,
        resources,
        registry: Arc::new(Registry::new()),
        blobl_env: ekiben_bloblang::Environment::new(),
        metrics: StreamMetrics::new("kafka-test", "org-kafka"),
        label: String::new(),
        path: "root.input".into(),
        file_root: None,
        env_vars: Arc::new(HashMap::new()),
        shutdown_rx: tokio::sync::watch::channel(false).1,
    }
}

pub(crate) fn conf(v: JsonValue) -> ComponentConfig {
    ComponentConfig {
        label: String::new(),
        component_type: "kafka_franz".into(),
        config: v,
        processors: Vec::new(),
    }
}

#[test]
fn bento_defaults_parse() {
    let s = parse(&json!({
        "seed_brokers": ["kafka-1.example.org:9092,kafka-2.example.org", "[2001:db8::1]:9093"],
        "topics": ["vehicle-positions"],
        "consumer_group": "traffic",
    }))
    .expect("parses");
    assert_eq!(
        s.seeds,
        vec![
            ("kafka-1.example.org".into(), 9092),
            ("kafka-2.example.org".into(), 9092),
            ("[2001:db8::1]".into(), 9093)
        ]
    );
    assert_eq!(s.client_id, "bento");
    assert_eq!(s.checkpoint_limit, 1024);
    assert!(s.auto_replay_nacks && s.oldest && !s.read_committed && s.tls.is_none());
    assert_eq!(s.commit_period, Duration::from_secs(5));
    assert_eq!(s.fetch_max_bytes, 50 << 20);
    assert_eq!(s.fetch_max_partition_bytes, 1 << 20);
    assert_eq!(s.broker_read_max_bytes, 100 << 20);
}

#[test]
fn the_form_s_empty_fields_are_accepted() {
    parse(&json!({
        "seed_brokers": ["kafka.example.org:9092"],
        "topics": ["a"],
        "rack_id": "",
        "rate_limit": "",
        "batching": {},
        "group_balancers": ["cooperative_sticky", "range"],
        "tls": {"enabled": false, "skip_cert_verify": false, "root_cas": "", "client_certs": []},
        "sasl": [],
        "auto_offset_reset": "latest",
        "transaction_isolation_level": "read_committed",
    }))
    .expect("parses");
}

#[test]
fn unsafe_and_unsupported_settings_are_refused_with_a_fix() {
    let base = || json!({"seed_brokers": ["kafka.example.org:9092"], "topics": ["a"]});
    let with = |k: &str, v: JsonValue| {
        let mut c = base();
        c[k] = v;
        c
    };
    for (c, needle) in [
        (
            with("tls", json!({"enabled": true, "skip_cert_verify": true})),
            "always verifies",
        ),
        (
            with(
                "tls",
                json!({"enabled": true, "root_cas_file": "/etc/ca.pem"}),
            ),
            "tls.root_cas",
        ),
        (
            with(
                "tls",
                json!({"enabled": true, "client_certs": [{"cert": "x"}]}),
            ),
            "client_certs",
        ),
        (
            with("tls", json!({"enabled": true, "root_cas": "not pem"})),
            "no PEM certificate",
        ),
        (
            with(
                "sasl",
                json!([{"mechanism": "PLAIN", "username": "u", "password": "p"}]),
            ),
            "in the clear",
        ),
        (
            with("sasl", json!([{"mechanism": "AWS_MSK_IAM"}])),
            "not supported",
        ),
        (
            with(
                "sasl",
                json!([{"mechanism": "SCRAM-SHA-512", "username": "u"}]),
            ),
            "${KAFKA_PASSWORD}",
        ),
        (
            with(
                "sasl",
                json!([{"mechanism": "SCRAM-SHA-512", "username": "u", "password": "p"}, {"mechanism": "SCRAM-SHA-256", "username": "u", "password": "p"}]),
            ),
            "more than one",
        ),
        (with("seed_brokers", json!([])), "at least one broker"),
        (
            with("seed_brokers", json!(["http://kafka.example.org:9092"])),
            "host:port",
        ),
        (
            with("seed_brokers", json!(["user@kafka.example.org:9092"])),
            "host:port",
        ),
        (with("topics", json!(["a:0"])), "explicit partitions"),
        (with("topics", json!(["a b"])), "topic name"),
        (with("rack_id", json!("eu-1a")), "rack_id"),
        (with("batching", json!({"count": 10})), "batching"),
        (with("group_balancers", json!(["fastest"])), "range"),
        (
            with("auto_offset_reset", json!("none")),
            "earliest or latest",
        ),
        (
            with("broker_read_max_bytes", json!("1GiB")),
            "between 1 byte and 256 MiB",
        ),
        (
            with("fetch_max_bytes", json!("120MiB")),
            "larger than broker_read_max_bytes",
        ),
        (
            with("fetch_max_partition_bytes", json!("lots")),
            "e.g. 1MiB",
        ),
        (with("checkpoint_limit", json!(0)), "checkpoint_limit"),
        (with("commit_period", json!("soon")), "duration"),
        (
            with("transaction_isolation_level", json!("serializable")),
            "read_committed",
        ),
        (with("unknown", json!(1)), "field unknown not recognised"),
        (
            with("regexp_topics", json!("yes")),
            "regexp_topics must be true or false",
        ),
        (
            with("fetch_max_bytes", json!(-1)),
            "between 1 byte and 128 MiB",
        ),
        (
            with("fetch_max_bytes", json!(true)),
            "fetch_max_bytes is not a size",
        ),
        (
            with("fetch_max_bytes", json!("99999999999999999999GiB")),
            "fetch_max_bytes is not a size",
        ),
        (with("topics", json!([1])), "topics[0] must be a string"),
        (with("topics", json!(["a".repeat(250)])), "at most 249"),
        (
            with("preferring_lag", json!(100)),
            "reorder partitions by lag",
        ),
        (
            with("new_root_span_with_link", json!(true)),
            "tracing spans",
        ),
        (
            with("extract_tracing_map", json!(["root.trace"])),
            "tracing spans",
        ),
        (
            with("tls", json!({"bogus": 1})),
            "tls.bogus is not a field of tls",
        ),
        (
            with("tls", json!({"enable_renegotiation": true})),
            "renegotiation is not supported",
        ),
        (with("tls", json!(true)), "tls must be an object"),
        (with("sasl", json!({})), "sasl must be a list of mechanisms"),
        (with("sasl", json!(["PLAIN"])), "sasl[0] must be an object"),
        (
            with("sasl", json!([{"mechanism": "PLAIN", "bogus": 1}])),
            "sasl[0].bogus is not a field of sasl",
        ),
        (
            with("sasl", json!([{"mechanism": "GSSAPI"}])),
            "GSSAPI is not a SASL mechanism",
        ),
        (
            with("consumer_group", json!("g\n")),
            "consumer_group must be a name",
        ),
        (
            with("client_id", json!("a".repeat(201))),
            "client_id must be a name",
        ),
        (
            {
                let mut c = with("topics", json!(["("]));
                c["regexp_topics"] = json!(true);
                c
            },
            "topics: ( is not a regular expression",
        ),
        // T-7799: compiles only inside the `^(?:...)$` wrapper
        (
            {
                let mut c = with("topics", json!(["a)|(.*"]));
                c["regexp_topics"] = json!(true);
                c
            },
            "topics: a)|(.* is not a regular expression",
        ),
    ] {
        let e = refusal(c.clone());
        assert!(e.contains(needle), "{c}: {e}");
        assert!(e.starts_with("kafka_franz input: "), "{e}");
    }
    let e = refusal(json!({"topics": ["a"]}));
    assert!(e.contains("seed_brokers is required"), "{e}");
    let e = refusal(json!(["k:9092"]));
    assert!(e.contains("expected object value"), "{e}");
}

#[test]
fn every_accepted_form_parses_to_its_setting() {
    let s = parse(&json!({
        "seed_brokers": ["[::1]", "kafka.example.org:9093"],
        "topics": ["a"],
        "checkpoint_limit": 10,
        "auto_offset_reset": "earliest",
        "start_from_oldest": false,
        "client_id": "ingest",
        "fetch_max_bytes": 1_048_576,
        "sasl": [{"mechanism": "none"}],
        "tls": {"enabled": true},
        "multi_header": true,
    }))
    .expect("parses");
    assert_eq!(
        s.seeds,
        vec![("[::1]".into(), 9092), ("kafka.example.org".into(), 9093)]
    );
    assert_eq!(
        (s.checkpoint_limit, s.oldest, s.client_id.as_str()),
        (10, true, "ingest")
    );
    assert_eq!(s.fetch_max_bytes, 1 << 20);
    assert!(s.sasl.is_none() && s.multi_header);
    assert_eq!(s.tls, Some(Vec::new()), "the system roots alone");

    let pem = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
        .expect("cert")
        .cert
        .pem();
    let s = parse(&json!({
        "seed_brokers": ["kafka.example.org"],
        "topics": ["a"],
        "tls": {"enabled": true, "root_cas": pem},
        "sasl": [{"mechanism": "PLAIN", "username": "u", "password": "p"}],
    }))
    .expect("PLAIN over TLS");
    assert_eq!(s.tls.map(|t| t.len()), Some(1));
    assert_eq!(
        s.sasl.map(|x| x.mechanism),
        Some(crate::client::Mechanism::Plain)
    );
}

#[test]
fn sizes_take_decimal_and_binary_units() {
    for (text, n) in [
        ("10", 10),
        ("10b", 10),
        ("2kb", 2000),
        ("2KiB", 2048),
        ("3MB", 3_000_000),
        ("3mib", 3 << 20),
        ("1gb", 1_000_000_000),
        ("1GiB", 1 << 30),
    ] {
        let obj = json!({"k": text});
        let obj = obj.as_object().expect("object");
        assert_eq!(byte_size(obj, "k", 1, 1 << 40), Ok(n), "{text}");
    }
    let obj = json!({"k": "1 parsec"});
    assert!(byte_size(obj.as_object().expect("object"), "k", 1, 1 << 40).is_err());
}

#[test]
fn broker_addresses_take_a_host_an_ipv6_address_or_a_port() {
    assert_eq!(broker_address("[::1]"), Some(("[::1]".into(), 9092)));
    assert_eq!(broker_address("[::1]:9093"), Some(("[::1]".into(), 9093)));
    for bad in ["[::1]x", "[::1]:x", "a:b:9092", "", "k:0", ":9092", "k:x"] {
        assert_eq!(broker_address(bad), None, "{bad}");
    }
}

#[test]
fn the_password_stays_out_of_errors_and_debug() {
    let s = parse(&json!({
        "seed_brokers": ["kafka.example.org:9092"],
        "topics": ["a"],
        "tls": {"enabled": true},
        "sasl": [{"mechanism": "PLAIN", "username": "city", "password": "hunter2-secret"}],
    }))
    .expect("parses");
    let sasl = s.sasl.expect("sasl");
    assert_eq!(sasl.password.expose(), "hunter2-secret");
    assert!(!format!("{:?}", sasl.password).contains("hunter2"));
}

#[test]
fn regexp_topics_compile_anchored() {
    let s = parse(&json!({
        "seed_brokers": ["kafka.example.org"],
        "topics": ["traffic-.*"],
        "regexp_topics": true,
    }))
    .expect("parses");
    let Topics::Patterns(p) = s.topics else {
        panic!("patterns")
    };
    assert!(p[0].is_match("traffic-north"));
    assert!(!p[0].is_match("old-traffic-north"));
}

/// A seed (and so any advertised broker, which goes through the same connect) on a cloud
/// metadata or loopback address is refused before a byte is sent.
#[tokio::test]
async fn the_outbound_rule_refuses_internal_brokers() {
    for broker in ["169.254.169.254:9092", "127.0.0.1:9092", "localhost:9092"] {
        let c = ctx(OrgQuotas::default());
        let mut input = KafkaInput::new(
            &conf(json!({"seed_brokers": [broker], "topics": ["a"], "consumer_group": "g"})),
            &c,
        )
        .expect("builds");
        match input.read_batch(&c).await {
            Err(CoreError::AccessDenied { reason, .. }) => {
                assert!(reason.contains("refused"), "{broker}: {reason}")
            }
            Err(e) => panic!("{broker}: {e}"),
            Ok(_) => panic!("{broker}: read"),
        }
    }
}

#[test]
fn the_registry_lints_the_input() {
    let r = Registry::new();
    register_all(&r);
    assert!(lint(&json!({"seed_brokers": ["k:9092"], "topics": ["a"]})).is_empty());
    assert_eq!(lint(&json!({"topics": ["a"]})).len(), 1);
}
