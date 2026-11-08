#![forbid(unsafe_code)]
//! A stored datasource record drives a real query: the factory builds each built-in driver
//! from a [`DatasourceRecord`] and the query reaches a local mock server (or the test
//! Postgres) through the outbound guard, with credentials from `secretRefs` only.

use chrono::Utc;
use daiya_model::{DatasourceRecord, InstallationId, InstanceId, OrgId, Subject};
use daiya_query::{
    DenyTokenExchanger, DriverDeps, DsError, MapSecretResolver, OutboundPolicy,
    PassthroughAudienceExchanger, Query, QueryCtx, ResourceReq, build_driver, guard_for,
};
use ipnet::IpNet;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const ORG: OrgId = OrgId::from_u128(42);

/// One-request-per-connection HTTP server on 127.0.0.1: answers 200 with the body of the
/// first route whose prefix the request path starts with (404 otherwise) and records every
/// request head.
async fn mock(routes: Vec<(&'static str, Value)>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = vec![0u8; 16384];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let head = String::from_utf8_lossy(&buf[..n]).into_owned();
            let path = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
            log.lock().push(head);
            let (status, body) = routes
                .iter()
                .find(|(prefix, _)| path.starts_with(prefix))
                .map_or((404, "{}".to_owned()), |(_, body)| (200, body.to_string()));
            let resp = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        }
    });
    (format!("http://127.0.0.1:{}", addr.port()), seen)
}

fn record(uid: &str, kind: &str, url: &str, json_data: Value) -> DatasourceRecord {
    let now = Utc::now();
    DatasourceRecord {
        installation_id: InstallationId::from_u128(1),
        org_id: ORG,
        instance_id: InstanceId::from_u128(1),
        project_id: None,
        uid: uid.into(),
        name: uid.into(),
        kind: kind.into(),
        access: "proxy".into(),
        url: url.into(),
        is_default: false,
        read_only: false,
        json_data,
        secret_refs: BTreeMap::new(),
        allow_hosts: Vec::new(),
        wasm_digest: None,
        version: 1,
        created_at: now,
        updated_at: now,
    }
}

fn loopback_policy() -> OutboundPolicy {
    OutboundPolicy {
        allow_http: true,
        allowed_networks: vec!["127.0.0.0/8".parse::<IpNet>().unwrap()],
        ..OutboundPolicy::default()
    }
}

fn deps(policy: OutboundPolicy, secrets: MapSecretResolver) -> DriverDeps {
    DriverDeps {
        secrets: Arc::new(secrets),
        token_exchanger: Arc::new(PassthroughAudienceExchanger),
        policy,
    }
}

fn ctx(subject: Subject) -> QueryCtx {
    QueryCtx::new(
        ORG,
        InstanceId::from_u128(1),
        subject,
        1_700_000_000_000,
        1_700_000_600_000,
    )
}

fn q(model: Value) -> Vec<Query> {
    vec![Query {
        ref_id: "A".into(),
        hide: false,
        model,
    }]
}

#[tokio::test]
async fn stored_prometheus_record_queries_the_mock_with_its_secret_ref() {
    let (url, seen) = mock(vec![(
        "/api/v1/query_range",
        json!({
            "status": "success",
            "data": { "resultType": "matrix", "result": [
                { "metric": { "__name__": "up", "job": "api" }, "values": [[1700000000, "1"], [1700000060, "0"]] }
            ]}
        }),
    )])
    .await;
    let mut rec = record("prom", "prometheus", &url, json!({ "timeInterval": "30s" }));
    rec.secret_refs.insert("token".into(), "prom-secret".into());
    let mut secrets = MapSecretResolver::new();
    secrets.insert(ORG, "prom-secret", "the-bearer-value");
    let driver = build_driver(&rec, &deps(loopback_policy(), secrets))
        .await
        .unwrap();
    assert_eq!(driver.kind(), "prometheus");

    let sets = driver
        .query(
            &ctx(Subject::anonymous()),
            &q(json!({ "expr": "up", "legendFormat": "{{job}}" })),
        )
        .await
        .unwrap();
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0].frames.len(), 1);
    assert_eq!(sets[0].frames[0].fields.len(), 2);
    let head = seen.lock()[0].to_ascii_lowercase();
    assert!(
        head.starts_with("get /api/v1/query_range?query=up"),
        "{head}"
    );
    // timeInterval 30s sets the minimum step.
    assert!(head.contains("&step=30.000"), "{head}");
    assert!(
        head.contains("authorization: bearer the-bearer-value"),
        "{head}"
    );
}

#[tokio::test]
async fn stored_ngsild_record_exchanges_the_caller_token_for_the_endpoint() {
    let (url, seen) = mock(vec![(
        "/api/endpoint/air/ngsi-ld/v1/entities",
        json!([{ "id": "urn:ngsi-ld:AirQualityObserved:1", "type": "AirQualityObserved",
                 "pm10": { "type": "Property", "value": 12.5 } }]),
    )])
    .await;
    let rec = record(
        "air",
        "ngsild-grafana-datasource",
        &format!("{url}/api/endpoint/air"),
        json!({ "oauthPassThru": true, "pageSize": 50, "tenant": "city", "contextUrl": "https://ctx.example/c.jsonld" }),
    );
    let driver = build_driver(&rec, &deps(loopback_policy(), MapSecretResolver::new()))
        .await
        .unwrap();
    assert_eq!(driver.kind(), "ngsild");
    let caller = Subject::authenticated("alice", vec![ORG], vec![]).with_bearer_token("alice-jwt");
    let sets = driver
        .query(
            &ctx(caller),
            &q(json!({ "queryType": "entity", "entityType": "AirQualityObserved", "attributes": ["pm10"] })),
        )
        .await
        .unwrap();
    assert!(serde_json::to_string(&sets).unwrap().contains("12.5"));
    let head = seen.lock()[0].clone();
    let lower = head.to_ascii_lowercase();
    assert!(
        lower.contains("authorization: bearer exchanged:air:alice-jwt"),
        "{head}"
    );
    assert!(lower.contains("ngsild-tenant: city"), "{head}");
    assert!(lower.contains("limit=50"), "{head}");
    assert!(head.contains("https://ctx.example/c.jsonld"), "{head}");

    // Without a token exchanger the caller's token never leaves daiya: the query is refused.
    let deny = DriverDeps {
        secrets: Arc::new(MapSecretResolver::new()),
        token_exchanger: Arc::new(DenyTokenExchanger),
        policy: loopback_policy(),
    };
    let refused = build_driver(&rec, &deny).await.unwrap();
    let caller = Subject::authenticated("alice", vec![ORG], vec![]).with_bearer_token("alice-jwt");
    let err = refused
        .query(
            &ctx(caller),
            &q(json!({ "entityType": "AirQualityObserved" })),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, DsError::TokenExchange { .. }), "{err}");
    assert_eq!(seen.lock().len(), 1, "no second request left the host");
}

#[tokio::test]
async fn stored_odata_record_queries_with_the_record_audience() {
    let (url, seen) = mock(vec![(
        "/odata/v4/Datasets",
        json!({ "value": [ { "name": "a", "count": 3 }, { "name": "b", "count": 4 } ] }),
    )])
    .await;
    let rec = record(
        "catalog",
        "odata",
        &format!("{url}/odata/v4"),
        json!({ "audience": "dataroom" }),
    );
    let driver = build_driver(&rec, &deps(loopback_policy(), MapSecretResolver::new()))
        .await
        .unwrap();
    let caller = Subject::authenticated("bob", vec![ORG], vec![]).with_bearer_token("bob-jwt");
    let sets = driver
        .query(
            &ctx(caller),
            &q(json!({ "entitySet": "Datasets", "$top": 2 })),
        )
        .await
        .unwrap();
    assert_eq!(sets[0].frames[0].fields.len(), 2);
    let head = seen.lock()[0].to_ascii_lowercase();
    assert!(
        head.contains("authorization: bearer exchanged:dataroom:bob-jwt"),
        "{head}"
    );
    assert!(head.starts_with("get /odata/v4/datasets?$top=2"), "{head}");
}

#[tokio::test]
async fn the_guard_of_a_built_driver_refuses_what_the_operator_did_not_open() {
    let (url, seen) = mock(vec![("/", json!({}))]).await;
    let rec = record("prom", "prometheus", &url, json!({}));

    // Default policy: https only.
    let driver = build_driver(
        &rec,
        &deps(OutboundPolicy::default(), MapSecretResolver::new()),
    )
    .await
    .unwrap();
    let err = driver
        .query(&ctx(Subject::anonymous()), &q(json!({ "expr": "up" })))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("scheme"), "{err}");

    // http allowed but loopback not opened: the address is refused.
    let http_only = OutboundPolicy {
        allow_http: true,
        ..OutboundPolicy::default()
    };
    let driver = build_driver(&rec, &deps(http_only, MapSecretResolver::new()))
        .await
        .unwrap();
    let err = driver
        .query(&ctx(Subject::anonymous()), &q(json!({ "expr": "up" })))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("loopback"), "{err}");

    // An explicit allow-list that does not hold the URL's host refuses the call.
    let mut listed = rec.clone();
    listed.allow_hosts = vec!["prometheus.example.com".into()];
    let driver = build_driver(&listed, &deps(loopback_policy(), MapSecretResolver::new()))
        .await
        .unwrap();
    let err = driver
        .query(&ctx(Subject::anonymous()), &q(json!({ "expr": "up" })))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("allow-list"), "{err}");
    assert!(seen.lock().is_empty(), "nothing left the host");

    // The metadata address stays blocked inside an opened network.
    let meta = record(
        "m",
        "prometheus",
        "http://169.254.169.254/latest",
        json!({}),
    );
    let link_local = OutboundPolicy {
        allow_http: true,
        allowed_networks: vec!["169.254.0.0/16".parse().unwrap()],
        ..OutboundPolicy::default()
    };
    let guard = guard_for(&meta, &link_local, 80).unwrap();
    let err = guard
        .resolve_checked("169.254.169.254", 80)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("metadata"), "{err}");
    assert!(
        guard.resolve_checked("169.254.10.10", 80).await.is_err(),
        "not in allowHosts"
    );
    let mut other = meta.clone();
    other.allow_hosts = vec!["169.254.10.10".into()];
    let guard = guard_for(&other, &link_local, 80).unwrap();
    assert!(guard.resolve_checked("169.254.10.10", 80).await.is_ok());
    let mapped = guard_for(
        &record(
            "v6",
            "prometheus",
            "http://[::ffff:169.254.169.254]/",
            json!({}),
        ),
        &link_local,
        80,
    )
    .unwrap();
    assert!(
        mapped
            .check_ip("::ffff:169.254.169.254".parse().unwrap())
            .is_err()
    );
    assert!(mapped.check_ip("fd00:ec2::254".parse().unwrap()).is_err());
}

#[tokio::test]
async fn bad_records_and_kinds_without_a_driver_are_refused_with_a_reason() {
    let d = deps(loopback_policy(), MapSecretResolver::new());
    let cases: Vec<(DatasourceRecord, &str)> = vec![
        (record("e", "prometheus", "", json!({})), "url is empty"),
        (record("e", "prometheus", "http://", json!({})), "not valid"),
        (
            record("u", "influxdb", "https://x.example", json!({})),
            "influxdb",
        ),
        (
            record("w", "wasm:sample", "https://x.example", json!({})),
            "WASM",
        ),
        (
            record(
                "p",
                "postgres",
                "db.example:5432",
                json!({ "user": "u", "allowedViews": ["v"] }),
            ),
            "database",
        ),
        (
            record(
                "p",
                "postgres",
                "db.example:5432",
                json!({ "database": "d", "allowedViews": ["v"] }),
            ),
            "user",
        ),
        (
            record(
                "p",
                "postgres",
                "db.example:5432",
                json!({ "database": "d", "user": "u" }),
            ),
            "allowedViews",
        ),
        (
            record(
                "p",
                "postgres",
                "db.example:5432",
                json!({ "database": "d", "user": "u", "allowedViews": ["v"], "sslmode": "verify-full" }),
            ),
            "sslmode",
        ),
        (
            record(
                "p",
                "grafana-postgresql-datasource",
                "127.0.0.1:1",
                json!({ "database": "d", "user": "u", "allowedViews": [" "] }),
            ),
            "allowedViews",
        ),
    ];
    for (rec, needle) in cases {
        let err = match build_driver(&rec, &d).await {
            Ok(_) => panic!("{} ({}) built", rec.uid, rec.kind),
            Err(e) => e,
        };
        assert!(err.to_string().contains(needle), "{needle}: {err}");
    }
    // A password secretRef that the organization does not hold fails at build time.
    let mut missing = record(
        "p",
        "postgres",
        "127.0.0.1:5432",
        json!({ "database": "d", "user": "u", "allowedViews": ["v"] }),
    );
    missing
        .secret_refs
        .insert("password".into(), "absent".into());
    let err = match build_driver(&missing, &d).await {
        Ok(_) => panic!("built without its password"),
        Err(e) => e,
    };
    assert!(matches!(err, DsError::SecretResolution { .. }), "{err}");

    // A URL without a host names nothing to guard.
    let err = match build_driver(&record("f", "prometheus", "file:///etc/x", json!({})), &d).await {
        Ok(_) => panic!("built without a host"),
        Err(e) => e,
    };
    assert!(err.to_string().contains("no host"), "{err}");

    // The other supported sslmodes build (the pool connects lazily, on first use).
    for mode in ["allow", "require", "prefer"] {
        let rec = record(
            "p",
            "postgres",
            "127.0.0.1:5432",
            json!({ "database": "d", "user": "u", "allowedViews": ["v"], "sslmode": mode }),
        );
        assert!(build_driver(&rec, &d).await.is_ok(), "{mode}");
    }

    // The JSON-LD context may be given as `@context`.
    let (url, seen) = mock(vec![("/ngsi-ld/v1/entities", json!([]))]).await;
    let ngsi = build_driver(
        &record(
            "n",
            "ngsild",
            &url,
            json!({ "@context": "https://ctx.example/a.jsonld" }),
        ),
        &d,
    )
    .await
    .unwrap();
    ngsi.query(
        &ctx(Subject::anonymous()),
        &q(json!({ "queryType": "entity", "entityType": "T" })),
    )
    .await
    .unwrap();
    assert!(seen.lock()[0].contains("<https://ctx.example/a.jsonld>"));

    // Loki is a slot: it builds, and answers that it is not implemented.
    let loki = build_driver(&record("l", "loki", "https://loki.example", json!({})), &d)
        .await
        .unwrap();
    assert_eq!(loki.kind(), "loki");
    let err = loki
        .resource(
            &ctx(Subject::anonymous()),
            ResourceReq {
                path: "labels".into(),
                params: BTreeMap::new(),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, DsError::NotImplemented(_)), "{err}");
}

/// The Postgres driver against a real server: a view granted to a reader role, reached at the
/// guard-checked address (the test server sits on a private network the policy opens).
#[tokio::test]
async fn stored_postgres_record_runs_a_checked_select_on_a_real_server() {
    let Ok(admin_url) = std::env::var("DAIYA_TEST_DATABASE_URL") else {
        println!("SKIP: DAIYA_TEST_DATABASE_URL is unset");
        return;
    };
    let opts: sqlx::postgres::PgConnectOptions = admin_url.parse().unwrap();
    let admin = sqlx::PgPool::connect_with(opts.clone()).await.unwrap();
    let password = format!("{}", uuid::Uuid::new_v4().simple());
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "DO $$ BEGIN
            IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'daiya_ds_reader') THEN
                CREATE ROLE daiya_ds_reader LOGIN NOSUPERUSER NOBYPASSRLS;
            END IF;
         END $$;
         ALTER ROLE daiya_ds_reader PASSWORD '{password}';
         CREATE TABLE IF NOT EXISTS daiya_ds_secret (v TEXT);
         CREATE OR REPLACE VIEW daiya_ds_metrics AS
             SELECT TIMESTAMPTZ '2023-11-14 22:13:20+00' + (n || ' minutes')::interval AS time,
                    n::float8 AS value
             FROM generate_series(0, 4) AS n;
         GRANT SELECT ON daiya_ds_metrics TO daiya_ds_reader;"
    )))
    .execute(&admin)
    .await
    .unwrap();

    let host = opts.get_host().to_owned();
    let mut rec = record(
        "pg",
        "grafana-postgresql-datasource",
        &format!("{host}:{}", opts.get_port()),
        json!({
            "database": opts.get_database().unwrap_or("postgres"),
            "user": "daiya_ds_reader",
            "allowedViews": ["daiya_ds_metrics"],
            "sslmode": "disable"
        }),
    );
    rec.secret_refs
        .insert("password".into(), "pg-reader".into());
    let mut secrets = MapSecretResolver::new();
    secrets.insert(ORG, "pg-reader", password.clone());
    let ip: std::net::IpAddr = host
        .parse()
        .unwrap_or(std::net::IpAddr::from([127, 0, 0, 1]));
    let policy = OutboundPolicy {
        allowed_networks: vec![IpNet::from(ip)],
        ..OutboundPolicy::default()
    };
    let driver = build_driver(&rec, &deps(policy, secrets.clone()))
        .await
        .unwrap();
    assert_eq!(driver.kind(), "postgres");

    let sets = driver
        .query(
            &ctx(Subject::anonymous()),
            &q(json!({ "rawSql": "SELECT time, value FROM daiya_ds_metrics ORDER BY time", "format": "time_series" })),
        )
        .await
        .unwrap();
    let frame = &sets[0].frames[0];
    assert_eq!(frame.fields.len(), 2);
    assert_eq!(frame.fields[0].name, "Time");
    assert_eq!(frame.fields[1].values.len(), 5);
    // A long result (a string column beside the value) comes back wide, a field per label set.
    let sets = driver
        .query(
            &ctx(Subject::anonymous()),
            &q(json!({ "rawSql": "SELECT time, CASE WHEN value < 2 THEN 'low' ELSE 'high' END AS band, value FROM daiya_ds_metrics ORDER BY time", "format": "time_series" })),
        )
        .await
        .unwrap();
    let fields = &sets[0].frames[0].fields;
    assert_eq!(fields.len(), 3, "{fields:?}");
    assert_eq!(
        fields[1].labels.get("band").map(String::as_str),
        Some("low")
    );
    assert_eq!(
        fields[2].labels.get("band").map(String::as_str),
        Some("high")
    );
    assert_eq!(fields[1].values[4], serde_json::Value::Null);

    // A relation outside the granted views is refused before anything runs, and the
    // database itself refuses what the reader role was never granted.
    let err = driver
        .query(
            &ctx(Subject::anonymous()),
            &q(json!({ "rawSql": "SELECT v FROM daiya_ds_secret" })),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("allow-list"), "{err}");
    let mut overlisted = rec.clone();
    overlisted.uid = "pg-over".into();
    overlisted.json_data["allowedViews"] = json!(["daiya_ds_metrics", "daiya_ds_secret"]);
    let ip_policy = OutboundPolicy {
        allowed_networks: vec![IpNet::from(
            host.parse::<std::net::IpAddr>()
                .unwrap_or(std::net::IpAddr::from([127, 0, 0, 1])),
        )],
        ..OutboundPolicy::default()
    };
    let over = build_driver(&overlisted, &deps(ip_policy, secrets.clone()))
        .await
        .unwrap();
    let err = over
        .query(
            &ctx(Subject::anonymous()),
            &q(json!({ "rawSql": "SELECT v FROM daiya_ds_secret" })),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("permission denied"), "{err}");
    assert!(!err.to_string().contains(&password));
    let health = driver.health(&ctx(Subject::anonymous())).await;
    assert_eq!(
        health.status,
        daiya_query::HealthState::Ok,
        "{}",
        health.message
    );

    // Without the private network opened, the same record does not get built.
    let err = match build_driver(&rec, &deps(OutboundPolicy::default(), secrets)).await {
        Ok(_) => panic!("built to a private address"),
        Err(e) => e,
    };
    assert!(matches!(err, DsError::GuardRefused(_)), "{err}");
}
