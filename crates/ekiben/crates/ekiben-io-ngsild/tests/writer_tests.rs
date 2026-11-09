//! E5: the `ngsi_ld` processor and output against the in-process fake Endpoint.

mod common;

use std::time::{Duration, Instant};

use common::{capture_logs, conf, ctx, entity, FakeBroker, FOUR_MIB};
use ekiben_bloblang::{Environment, Value};
use ekiben_core::{Batch, ComponentConfig, CoreError, Output, Part, ProcContext, Processor};
use ekiben_io_ngsild::metrics;
use ekiben_io_ngsild::output::NgsiLdOutput;
use ekiben_io_ngsild::processor::NgsiLdProcessor;
use serde_json::{json, Value as JsonValue};

fn component(config: JsonValue, label: &str) -> ComponentConfig {
    ComponentConfig {
        label: label.to_string(),
        component_type: "ngsi_ld".to_string(),
        config,
        processors: Vec::new(),
    }
}

fn processor(config: JsonValue, pctx: &ProcContext) -> NgsiLdProcessor {
    NgsiLdProcessor::from_config(&component(config, "write_0"), pctx).expect("processor builds")
}

fn oauth(fake: &FakeBroker) -> JsonValue {
    json!({
        "enabled": true,
        "token_url": fake.token_url(),
        "client_key": "pl-traffic",
        "client_secret": "hunter2-client-secret"
    })
}

fn part(v: JsonValue) -> Part {
    Part::from_str(&v.to_string())
}

async fn run(p: &NgsiLdProcessor, pctx: &ProcContext, batch: Batch) -> Batch {
    let mut out = p.process_batch(pctx, batch).await.expect("process");
    assert_eq!(out.len(), 1);
    out.remove(0)
}

#[tokio::test]
async fn upsert_posts_to_the_endpoint_with_the_pipeline_token() {
    let fake = FakeBroker::start().await;
    fake.with(|s| s.require_token = true);
    let (_s, pctx, _stop) = ctx("org-a", true);
    let p = processor(conf(&fake.base(), json!({"oauth2": oauth(&fake)})), &pctx);
    let out = run(&p, &pctx, vec![part(entity("urn:ngsi-ld:Vehicle:1"))]).await;

    let posts = fake.posts();
    assert_eq!(posts.len(), 1);
    let r = &posts[0];
    assert_eq!(
        r.path(),
        "/api/endpoint/test/ngsi-ld/v1/entityOperations/upsert"
    );
    assert_eq!(r.query(), "options=update");
    assert_eq!(
        r.header("content-type").as_deref(),
        Some("application/json")
    );
    assert_eq!(r.header("accept").as_deref(), Some("application/json"));
    assert_eq!(r.header("authorization").as_deref(), Some("Bearer tok-1"));
    assert!(
        r.header("link").is_none(),
        "the gateway refuses a non-core @context"
    );
    assert!(
        r.header("ngsild-tenant").is_none(),
        "the gateway sets the tenant"
    );
    assert!(
        r.header("content-length").is_some(),
        "Antares answers 411 without it"
    );
    assert_eq!(r.json(), json!([entity("urn:ngsi-ld:Vehicle:1")]));
    assert_eq!(
        fake.with(|s| s.token_auth[0].clone()),
        "Basic cGwtdHJhZmZpYzpodW50ZXIyLWNsaWVudC1zZWNyZXQ="
    );

    assert_eq!(out[0].meta_get_str("jc_refused_write_0"), "[]");
    assert!(out[0].meta_get("jc_failed").is_none());
    assert!(!out[0].is_errored());
}

#[tokio::test]
async fn replace_sends_no_options_query() {
    let fake = FakeBroker::start().await;
    let (_s, pctx, _stop) = ctx("org-a", true);
    let p = processor(conf(&fake.base(), json!({"options": "replace"})), &pctx);
    run(&p, &pctx, vec![part(entity("urn:a"))]).await;
    let posts = fake.posts();
    assert_eq!(
        posts[0].path(),
        "/api/endpoint/test/ngsi-ld/v1/entityOperations/upsert"
    );
    assert_eq!(posts[0].query(), "");

    for (op, path) in [("update", "update"), ("create", "create")] {
        let fake = FakeBroker::start().await;
        let p = processor(conf(&fake.base(), json!({"operation": op})), &pctx);
        run(&p, &pctx, vec![part(entity("urn:a"))]).await;
        assert_eq!(
            fake.posts()[0].path(),
            format!("/api/endpoint/test/ngsi-ld/v1/entityOperations/{path}")
        );
        assert_eq!(fake.posts()[0].query(), "");
    }
}

#[tokio::test]
async fn chunks_by_1000_entities() {
    let fake = FakeBroker::start().await;
    let (_s, pctx, _stop) = ctx("org-a", true);
    let p = processor(
        conf(&fake.base(), json!({"max_batch_bytes": "4MiB"})),
        &pctx,
    );
    let batch: Batch = (0..2500)
        .map(|i| part(entity(&format!("urn:ngsi-ld:Vehicle:{i}"))))
        .collect();
    let out = run(&p, &pctx, batch).await;
    let sizes: Vec<usize> = fake
        .posts()
        .iter()
        .map(|r| r.json().as_array().map_or(0, Vec::len))
        .collect();
    assert_eq!(sizes, vec![1000, 1000, 500]);
    assert!(out
        .iter()
        .all(|p| p.meta_get_str("jc_refused_write_0") == "[]"));
}

#[tokio::test]
async fn chunks_by_bytes_under_the_4mib_limit() {
    let fake = FakeBroker::start().await;
    let (_s, pctx, _stop) = ctx("org-a", true);
    let p = processor(
        conf(&fake.base(), json!({"max_batch_bytes": "4MiB"})),
        &pctx,
    );
    let big = "x".repeat(300 * 1024);
    let batch: Batch = (0..30)
        .map(|i| part(json!({"id": format!("urn:e:{i}"), "type": "T", "blob": {"type": "Property", "value": big}})))
        .collect();
    let out = run(&p, &pctx, batch).await;
    let posts = fake.posts();
    assert!(posts.len() >= 3, "{} requests", posts.len());
    for r in &posts {
        assert!(r.body.len() <= FOUR_MIB, "a body of {} bytes", r.body.len());
    }
    let sent: usize = posts
        .iter()
        .map(|r| r.json().as_array().map_or(0, Vec::len))
        .sum();
    assert_eq!(sent, 30);
    assert!(out
        .iter()
        .all(|p| p.meta_get_str("jc_refused_write_0") == "[]"));
}

#[tokio::test]
async fn an_entity_over_the_byte_cap_is_a_member_refusal() {
    let fake = FakeBroker::start().await;
    let (_s, pctx, _stop) = ctx("org-a", true);
    let p = processor(
        conf(&fake.base(), json!({"max_batch_bytes": "1KiB"})),
        &pctx,
    );
    let big =
        json!({"id": "urn:big", "type": "T", "v": {"type": "Property", "value": "y".repeat(2048)}});
    let out = run(&p, &pctx, vec![part(json!([big, entity("urn:small")]))]).await;
    let refused: JsonValue =
        serde_json::from_str(&out[0].meta_get_str("jc_refused_write_0")).expect("json");
    assert_eq!(refused.as_array().map(Vec::len), Some(1));
    assert_eq!(refused[0]["id"], "urn:big");
    let posts = fake.posts();
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].json(), json!([entity("urn:small")]));
}

#[tokio::test]
async fn multi_status_refusals_land_on_their_messages() {
    let fake = FakeBroker::start().await;
    fake.refuse("urn:b", "speed must be a number");
    let (s, pctx, _stop) = ctx("org-a", true);
    let p = processor(conf(&fake.base(), json!({})), &pctx);
    let out = run(
        &p,
        &pctx,
        vec![
            part(entity("urn:a")),
            part(entity("urn:b")),
            part(json!([entity("urn:c"), entity("urn:d")])),
        ],
    )
    .await;
    assert_eq!(
        fake.posts().len(),
        1,
        "one request carries all four entities"
    );
    assert_eq!(out[0].meta_get_str("jc_refused_write_0"), "[]");
    assert_eq!(
        out[1].meta_get_str("jc_refused_write_0"),
        r#"[{"id":"urn:b","message":"speed must be a number"}]"#
    );
    assert_eq!(out[2].meta_get_str("jc_refused_write_0"), "[]");
    assert!(out.iter().all(|p| !p.is_errored()));
    assert_eq!(s.metrics.get_counter(&metrics::written_key("write_0")), 3);
    assert_eq!(s.metrics.get_counter(&metrics::refused_key("write_0")), 1);
    assert_eq!(
        s.metrics
            .get_counter(&metrics::requests_key("write_0", "207")),
        1
    );
}

#[tokio::test]
async fn metadata_equals_the_portal_result_map() {
    // The branch the processor replaces formats the 207 with this mapping (identity.rs).
    let body = r#"{"success":["urn:a"],"errors":[{"entityId":"urn:b","error":{"type":"x","title":"Bad Request","detail":"a <b> & c"}},{"entityId":"urn:c","error":{"type":"x","title":"Only title"}},{"entityId":"unknown","error":{"type":"x"}}]}"#;
    let mapping = r#"root = this.errors.or([]).map_each(e -> {"id": e.entityId.or(""), "message": e.error.detail.or(e.error.title).or("")}).format_json(no_indent: true)"#;
    let exec = Environment::new()
        .compile_mapping(mapping)
        .expect("mapping");
    let input = ekiben_bloblang::go_json::decode_with_numbers(body.as_bytes()).expect("json");
    let expected = match exec.exec_on_value(input).expect("runs") {
        Value::Bytes(b) => String::from_utf8(b).expect("utf8"),
        Value::String(s) => s,
        other => panic!("unexpected {other:?}"),
    };

    let fake = FakeBroker::start().await;
    let (_s, pctx, _stop) = ctx("org-a", true);
    let p = processor(conf(&fake.base(), json!({})), &pctx);
    fake.script(207, &[("content-type", "application/json")], body);
    let out = run(
        &p,
        &pctx,
        vec![part(json!([
            entity("urn:a"),
            entity("urn:b"),
            entity("urn:c")
        ]))],
    )
    .await;
    let got = out[0].meta_get_str("jc_refused_write_0");
    let got_v: JsonValue = serde_json::from_str(&got).expect("json");
    let exp_v: JsonValue = serde_json::from_str(&expected).expect("json");
    assert_eq!(got_v, exp_v, "got {got}, the Portal writes {expected}");
    assert_eq!(got, expected, "byte for byte");
}

#[tokio::test]
async fn a_whole_refusal_sets_jc_failed_only() {
    let fake = FakeBroker::start().await;
    let (_s, pctx, _stop) = ctx("org-a", true);
    let p = processor(conf(&fake.base(), json!({})), &pctx);
    fake.script(
        400,
        &[("content-type", "application/json")],
        r#"{"type":"x","title":"Bad Request","detail":"the entity has no type"}"#,
    );
    let out = run(&p, &pctx, vec![part(entity("urn:a"))]).await;
    assert_eq!(fake.posts().len(), 1, "drop_on is never retried");
    assert_eq!(out[0].meta_get_str("jc_failed"), "the entity has no type");
    assert!(out[0].meta_get("jc_refused_write_0").is_none());
    assert!(!out[0].is_errored());
}

#[tokio::test]
async fn a_packed_whole_refusal_is_isolated_per_message() {
    let fake = FakeBroker::start().await;
    let (_s, pctx, _stop) = ctx("org-a", true);
    let p = processor(conf(&fake.base(), json!({})), &pctx);
    let bad = r#"{"type":"x","title":"Bad Request","detail":"invalid"}"#;
    fake.script(400, &[], bad);
    fake.script(400, &[], bad);
    let out = run(
        &p,
        &pctx,
        vec![part(entity("urn:bad")), part(entity("urn:good"))],
    )
    .await;
    let posts = fake.posts();
    assert_eq!(posts.len(), 3);
    assert_eq!(posts[1].json(), json!([entity("urn:bad")]));
    assert_eq!(posts[2].json(), json!([entity("urn:good")]));
    assert_eq!(out[0].meta_get_str("jc_failed"), "invalid");
    assert_eq!(out[1].meta_get_str("jc_refused_write_0"), "[]");
    assert!(out[1].meta_get("jc_failed").is_none());
}

#[tokio::test]
async fn retry_after_on_429_is_honoured() {
    let fake = FakeBroker::start().await;
    let (_s, pctx, _stop) = ctx("org-a", true);
    let p = processor(conf(&fake.base(), json!({})), &pctx);
    fake.script(429, &[("retry-after", "1")], "slow down");
    let started = Instant::now();
    let out = run(&p, &pctx, vec![part(entity("urn:a"))]).await;
    assert!(started.elapsed() >= Duration::from_millis(900));
    assert_eq!(fake.posts().len(), 2);
    assert_eq!(out[0].meta_get_str("jc_refused_write_0"), "[]");
}

#[tokio::test]
async fn persistent_503_flags_the_message_for_replay() {
    let fake = FakeBroker::start().await;
    let (_s, pctx, _stop) = ctx("org-a", true);
    let p = processor(conf(&fake.base(), json!({"retries": 3})), &pctx);
    for _ in 0..4 {
        fake.script(503, &[("retry-after", "0")], "busy");
    }
    let out = run(&p, &pctx, vec![part(entity("urn:a"))]).await;
    assert_eq!(fake.posts().len(), 4, "retries 3 is four attempts");
    assert!(out[0].is_errored());
    assert!(out[0].meta_get("jc_failed").is_none());
    assert!(out[0].meta_get("jc_refused_write_0").is_none());
}

#[tokio::test]
async fn the_token_is_refreshed_before_it_expires() {
    let fake = FakeBroker::start().await;
    fake.with(|s| {
        s.require_token = true;
        s.token_ttl = 2;
    });
    let (_s, pctx, _stop) = ctx("org-refresh", true);
    let p = processor(conf(&fake.base(), json!({"oauth2": oauth(&fake)})), &pctx);
    run(&p, &pctx, vec![part(entity("urn:a"))]).await;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let out = run(&p, &pctx, vec![part(entity("urn:a"))]).await;
    assert_eq!(fake.with(|s| s.token_requests), 2);
    let posts = fake.posts();
    assert_eq!(posts.len(), 2, "no request was answered 401");
    assert_eq!(
        posts[1].header("authorization").as_deref(),
        Some("Bearer tok-2")
    );
    assert_eq!(out[0].meta_get_str("jc_refused_write_0"), "[]");
}

#[tokio::test]
async fn a_401_refreshes_the_token_once() {
    let fake = FakeBroker::start().await;
    fake.with(|s| s.require_token = true);
    let (_s, pctx, _stop) = ctx("org-401", true);
    let p = processor(
        conf(&fake.base(), json!({"oauth2": oauth(&fake), "retries": 0})),
        &pctx,
    );
    run(&p, &pctx, vec![part(entity("urn:a"))]).await;
    // Keycloak revoked the session early.
    fake.with(|s| s.tokens.clear());
    let out = run(&p, &pctx, vec![part(entity("urn:a"))]).await;
    assert_eq!(fake.posts().len(), 3);
    assert_eq!(fake.with(|s| s.token_requests), 2);
    assert_eq!(out[0].meta_get_str("jc_refused_write_0"), "[]");

    // A broker that refuses every token: one refresh, then the failure.
    fake.with(|s| s.token_ttl = 0);
    fake.with(|s| s.tokens.clear());
    let before = fake.posts().len();
    let out = run(&p, &pctx, vec![part(entity("urn:a"))]).await;
    assert_eq!(fake.posts().len() - before, 2);
    assert!(out[0].is_errored());
}

#[tokio::test]
async fn the_secret_and_the_token_never_reach_a_log_or_an_error() {
    let (logs, _guard) = capture_logs();
    let fake = FakeBroker::start().await;
    let (_s, pctx, _stop) = ctx("org-logs", true);
    let p = processor(
        conf(&fake.base(), json!({"oauth2": oauth(&fake), "retries": 1})),
        &pctx,
    );
    fake.script(503, &[("retry-after", "0")], "down");
    fake.script(503, &[("retry-after", "0")], "down");
    let out = run(&p, &pctx, vec![part(entity("urn:a"))]).await;
    assert!(out[0].is_errored());
    let err = out[0]
        .error_get()
        .map(|e| e.to_string())
        .unwrap_or_default();
    let text = logs.text();
    for secret in ["hunter2-client-secret", "tok-1", "cGwtdHJhZmZpYzpodW50ZXIy"] {
        assert!(!err.contains(secret), "error: {err}");
        assert!(!text.contains(secret), "logs: {text}");
    }
    assert!(text.contains("retrying"), "the retry is logged: {text}");
}

#[tokio::test]
async fn the_output_dead_letters_refused_members_and_acks_the_rest() {
    let fake = FakeBroker::start().await;
    fake.refuse("urn:b", "bad");
    let (sctx, _p, _stop) = ctx("org-a", true);
    let out = NgsiLdOutput::from_config(&component(conf(&fake.base(), json!({})), "sink"), &sctx)
        .expect("output builds");
    let res = out
        .write_batch(
            &sctx,
            vec![
                part(entity("urn:a")),
                part(entity("urn:b")),
                part(entity("urn:c")),
            ],
        )
        .await;
    let Err(CoreError::Batch(err)) = res else {
        panic!("a member refusal is an indexed error, got {res:?}");
    };
    assert_eq!(err.indexed_errors(), 1);
    assert!(err
        .part_errors
        .get(&1)
        .is_some_and(|e| e.contains("urn:b: bad")));
    assert_eq!(
        err.errored_batch.len(),
        3,
        "the received batch, for fallback's sort groups"
    );

    let ok = out.write_batch(&sctx, vec![part(entity("urn:a"))]).await;
    assert!(ok.is_ok());
}

#[tokio::test]
async fn an_outside_broker_gets_link_context_and_tenant() {
    let fake = FakeBroker::start().await;
    let (_s, pctx, _stop) = ctx("org-a", true);
    let p = processor(
        conf(
            &fake.outside_base(),
            json!({"context": "https://example.org/ctx.jsonld", "tenant": "city"}),
        ),
        &pctx,
    );
    run(&p, &pctx, vec![part(entity("urn:a"))]).await;
    let r = &fake.posts()[0];
    assert_eq!(
        r.header("link").as_deref(),
        Some("<https://example.org/ctx.jsonld>; rel=\"http://www.w3.org/ns/json-ld#context\"; type=\"application/ld+json\"")
    );
    assert_eq!(r.header("ngsild-tenant").as_deref(), Some("city"));
    assert_eq!(
        r.header("content-type").as_deref(),
        Some("application/json")
    );
}

#[tokio::test]
async fn configs_the_platform_refuses_are_refused_at_build() {
    let fake = FakeBroker::start().await;
    let (_s, pctx, _stop) = ctx("org-a", true);
    let base = fake.base();
    let cases = [
        conf(
            &base,
            json!({"headers": {"Authorization": "Bearer static"}}),
        ),
        conf(&base, json!({"headers": {"NGSILD-Tenant": "other"}})),
        conf(&base, json!({"context": "https://example.org/ctx.jsonld"})),
        conf(&base, json!({"tenant": "other"})),
        conf(&base, json!({"max_batch_count": 1001})),
        conf(&base, json!({"max_batch_bytes": "5MiB"})),
        conf(
            &base,
            json!({"tls": {"enabled": true, "skip_cert_verify": true}}),
        ),
        conf(&base, json!({"operation": "create", "options": "replace"})),
        conf(&base, json!({"not_a_field": 1})),
        conf(
            &format!("http://user:pw@{}/api/endpoint/test/ngsi-ld/v1", fake.addr),
            json!({}),
        ),
        conf(&format!("http://{}/", fake.addr), json!({})),
        conf(
            &base,
            json!({"oauth2": {"enabled": true, "token_url": "file:///etc/passwd", "client_key": "k"}}),
        ),
    ];
    for c in cases {
        let built = NgsiLdProcessor::from_config(&component(c.clone(), "w"), &pctx);
        assert!(built.is_err(), "{c} must be refused");
    }
}

#[tokio::test]
async fn private_addresses_are_refused_before_any_request() {
    let fake = FakeBroker::start().await;
    let (_s, pctx, _stop) = ctx("org-strict", false);
    for base in [
        fake.base(),
        "http://169.254.169.254/ngsi-ld/v1".to_string(),
        "http://[::ffff:127.0.0.1]/ngsi-ld/v1".to_string(),
        "http://localhost/ngsi-ld/v1".to_string(),
    ] {
        let p = processor(conf(&base, json!({"retries": 0})), &pctx);
        let out = run(&p, &pctx, vec![part(entity("urn:a"))]).await;
        assert!(out[0].is_errored(), "{base} must be refused");
        let err = out[0]
            .error_get()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(
            err.contains("SSRF") || err.contains("access denied"),
            "{err}"
        );
    }
    assert!(fake.requests().is_empty(), "the fake was never called");
}

#[tokio::test]
async fn a_message_that_is_no_entity_is_refused_whole_locally() {
    let fake = FakeBroker::start().await;
    let (_s, pctx, _stop) = ctx("org-a", true);
    let p = processor(conf(&fake.base(), json!({})), &pctx);
    let out = run(
        &p,
        &pctx,
        vec![
            Part::from_str("\"text\""),
            Part::from_str("[]"),
            part(entity("urn:a")),
        ],
    )
    .await;
    assert!(out[0].meta_get_str("jc_failed").contains("NGSI-LD entity"));
    assert_eq!(out[1].meta_get_str("jc_refused_write_0"), "[]");
    assert_eq!(out[2].meta_get_str("jc_refused_write_0"), "[]");
    assert_eq!(fake.posts().len(), 1);
    assert_eq!(fake.posts()[0].json(), json!([entity("urn:a")]));
}

#[tokio::test]
async fn one_request_never_carries_an_id_twice() {
    let fake = FakeBroker::start().await;
    let (_s, pctx, _stop) = ctx("org-a", true);
    let p = processor(conf(&fake.base(), json!({})), &pctx);
    run(
        &p,
        &pctx,
        vec![
            part(entity("urn:a")),
            part(entity("urn:a")),
            part(entity("urn:b")),
        ],
    )
    .await;
    let sizes: Vec<usize> = fake
        .posts()
        .iter()
        .map(|r| r.json().as_array().map_or(0, Vec::len))
        .collect();
    assert_eq!(sizes, vec![1, 2]);
}

#[tokio::test]
async fn the_rate_limit_resource_is_required_and_used() {
    let fake = FakeBroker::start().await;
    let (_s, pctx, _stop) = ctx("org-a", true);
    let p = processor(
        conf(&fake.base(), json!({"rate_limit": "proj__pipeline_egress"})),
        &pctx,
    );
    let out = run(&p, &pctx, vec![part(entity("urn:a"))]).await;
    assert!(
        out[0].is_errored(),
        "a missing rate_limit resource is an error, never skipped"
    );
    assert!(fake.requests().is_empty());
}

// --- Bento ports: internal/impl/io/output_http_client_test.go ---

#[tokio::test]
async fn bento_output_http_client_test_http_client_retries() {
    let fake = FakeBroker::start().await;
    let (sctx, _p, _stop) = ctx("org-a", true);
    for _ in 0..4 {
        fake.script(403, &[], "test error");
    }
    let out = NgsiLdOutput::from_config(
        &component(
            conf(&fake.base(), json!({"retries": 3, "retry_period": "1ms"})),
            "o",
        ),
        &sctx,
    )
    .expect("builds");
    assert!(out
        .write_batch(&sctx, vec![part(entity("urn:a"))])
        .await
        .is_err());
    assert_eq!(fake.posts().len(), 4);
}

#[tokio::test]
async fn bento_output_http_client_test_http_client_basic() {
    let fake = FakeBroker::start().await;
    let (sctx, _p, _stop) = ctx("org-a", true);
    let out = NgsiLdOutput::from_config(&component(conf(&fake.base(), json!({})), "o"), &sctx)
        .expect("builds");
    for i in 0..50 {
        let id = format!("urn:test:{i}");
        out.write_batch(&sctx, vec![part(entity(&id))])
            .await
            .expect("written");
        let posts = fake.posts();
        assert_eq!(posts.len(), i + 1);
        assert_eq!(posts[i].json(), json!([entity(&id)]));
    }
}

// --- Bento ports: internal/httpclient/client_test.go ---

#[tokio::test]
async fn bento_client_test_http_client_retries() {
    let fake = FakeBroker::start().await;
    let (_s, pctx, _stop) = ctx("org-a", true);
    for _ in 0..4 {
        fake.script(403, &[], "test error");
    }
    let p = processor(
        conf(&fake.base(), json!({"retries": 3, "retry_period": "1ms"})),
        &pctx,
    );
    let out = run(&p, &pctx, vec![part(entity("urn:a"))]).await;
    assert!(out[0].is_errored());
    assert_eq!(fake.posts().len(), 4);
}

#[tokio::test]
async fn bento_client_test_http_client_bad_request() {
    let (_s, pctx, _stop) = ctx("org-a", true);
    let built = NgsiLdProcessor::from_config(
        &component(
            json!({"url": "htp://notvalid:1111", "retries": 3, "retry_period": "1ms"}),
            "w",
        ),
        &pctx,
    );
    assert!(built.is_err());
}

#[tokio::test]
async fn bento_client_test_http_client_send_basic() {
    let fake = FakeBroker::start().await;
    let (_s, pctx, _stop) = ctx("org-a", true);
    let p = processor(conf(&fake.base(), json!({})), &pctx);
    for i in 0..100 {
        let id = format!("urn:test:{i}");
        run(&p, &pctx, vec![part(entity(&id))]).await;
        assert_eq!(fake.posts()[i].json(), json!([entity(&id)]));
    }
}

#[tokio::test]
async fn bento_client_test_http_client_drop_on() {
    let fake = FakeBroker::start().await;
    let (sctx, _p, _stop) = ctx("org-a", true);
    fake.script(400, &[], r#"{"foo":"bar"}"#);
    let out = NgsiLdOutput::from_config(
        &component(conf(&fake.base(), json!({"drop_on": [400]})), "o"),
        &sctx,
    )
    .expect("builds");
    assert!(out
        .write_batch(&sctx, vec![part(entity("urn:a"))])
        .await
        .is_err());
    assert_eq!(fake.posts().len(), 1);
}

#[tokio::test]
async fn bento_client_test_http_client_successful_on() {
    let fake = FakeBroker::start().await;
    let (sctx, _p, _stop) = ctx("org-a", true);
    fake.script(400, &[], r#"{"foo":"bar"}"#);
    let out = NgsiLdOutput::from_config(
        &component(
            conf(&fake.base(), json!({"drop_on": [], "successful_on": [400]})),
            "o",
        ),
        &sctx,
    )
    .expect("builds");
    assert!(out
        .write_batch(&sctx, vec![part(entity("urn:a"))])
        .await
        .is_ok());
    assert_eq!(fake.posts().len(), 1);
}

#[tokio::test]
async fn bento_client_test_http_client_oauth2_conf() {
    let fake = FakeBroker::start().await;
    fake.with(|s| {
        s.token_form_answer = true;
        s.require_token = true;
    });
    let (_s, pctx, _stop) = ctx("org-oauth", true);
    let p = processor(
        conf(
            &fake.base(),
            json!({"oauth2": {"enabled": true, "token_url": fake.token_url(), "client_key": "fookey", "client_secret": "foosecret"}}),
        ),
        &pctx,
    );
    let out = run(&p, &pctx, vec![part(entity("urn:a"))]).await;
    assert!(!out[0].is_errored());
    assert_eq!(
        fake.with(|s| s.token_auth[0].clone()),
        "Basic Zm9va2V5OmZvb3NlY3JldA=="
    );
    assert_eq!(
        fake.with(|s| s.token_bodies[0].clone()),
        "grant_type=client_credentials"
    );
    assert_eq!(
        fake.posts()[0].header("authorization").as_deref(),
        Some("Bearer tok-1")
    );
}

/// jc-smoke-gate (smoke logs row 4): every batch names itself by `X-Request-Id`, one run id with
/// the batch's number, so the broker's line of the write joins the stream's; a retry keeps it.
#[tokio::test]
async fn each_batch_carries_one_request_id_of_its_run() {
    let fake = FakeBroker::start().await;
    let (_s, pctx, _stop) = ctx("org-a", true);
    let p = processor(conf(&fake.base(), json!({})), &pctx);
    let batch: Batch = (0..2500)
        .map(|i| part(entity(&format!("urn:ngsi-ld:Vehicle:{i}"))))
        .collect();
    run(&p, &pctx, batch).await;
    fake.script(503, &[("retry-after", "0")], "{}");
    run(&p, &pctx, vec![part(entity("urn:ngsi-ld:Vehicle:x"))]).await;
    let ids: Vec<String> = fake
        .posts()
        .iter()
        .map(|r| r.header("x-request-id").expect("every batch names itself"))
        .collect();
    assert_eq!(ids.len(), 5, "{ids:?}");
    let run_id = ids[0].rsplit_once('-').expect("<run>-<n>").0.to_owned();
    assert_eq!(run_id.len(), 32, "{ids:?}");
    let numbers: Vec<&str> = ids
        .iter()
        .map(|id| id.strip_prefix(&format!("{run_id}-")).expect("one run"))
        .collect();
    assert_eq!(
        numbers,
        ["0", "1", "2", "3", "3"],
        "the retry keeps its batch's id"
    );
    for id in &ids {
        assert!(
            (8..=64).contains(&id.len())
                && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
            "jc keeps only a plain id: {id}"
        );
    }
}
