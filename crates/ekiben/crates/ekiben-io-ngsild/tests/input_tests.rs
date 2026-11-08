//! E6: the `query` and `temporal` modes of the `ngsi_ld` input against the in-process fake.

mod common;

use std::time::Duration;

use common::{conf, ctx, FakeBroker};
use ekiben_core::{
    AckResponder, Batch, BatchError, ComponentConfig, CoreError, Input, StreamContext,
};
use ekiben_io_ngsild::input::NgsiLdInput;
use serde_json::{json, Value as JsonValue};

fn input(config: JsonValue, sctx: &StreamContext) -> Result<NgsiLdInput, CoreError> {
    NgsiLdInput::from_config(
        &ComponentConfig {
            label: "read".to_string(),
            component_type: "ngsi_ld".to_string(),
            config,
            processors: Vec::new(),
        },
        sctx,
    )
}

async fn read(
    i: &mut NgsiLdInput,
    sctx: &StreamContext,
) -> Result<(Batch, AckResponder), CoreError> {
    match tokio::time::timeout(Duration::from_secs(10), i.read_batch(sctx)).await {
        Ok(r) => r,
        Err(_) => panic!("read_batch did not answer"),
    }
}

fn ids(batch: &Batch) -> Vec<String> {
    batch
        .iter()
        .map(|p| {
            let v: JsonValue = serde_json::from_slice(&p.as_bytes_ref()).unwrap_or(JsonValue::Null);
            v["id"].as_str().unwrap_or("").to_string()
        })
        .collect()
}

fn vehicles(n: usize) -> Vec<JsonValue> {
    (0..n)
        .map(|i| json!({"id": format!("urn:ngsi-ld:Vehicle:{i}"), "type": "Vehicle", "speed": {"type": "Property", "value": i}}))
        .collect()
}

#[tokio::test]
async fn query_reads_the_portal_page_sequence() {
    let fake = FakeBroker::start().await;
    fake.with(|s| s.entities = vehicles(2500));
    let (sctx, _p, _stop) = ctx("org-q", true);
    let mut i = input(
        conf(
            &fake.base(),
            json!({"type": "Vehicle", "attrs": ["speed", "location"], "q": "speed>50"}),
        ),
        &sctx,
    )
    .expect("builds");
    i.connect(&sctx).await.expect("connects");
    let mut sizes = Vec::new();
    loop {
        match read(&mut i, &sctx).await {
            Ok((batch, ack)) => {
                sizes.push(batch.len());
                ack.ack();
            }
            Err(CoreError::EndOfInput) => break,
            Err(e) => panic!("{e}"),
        }
    }
    assert_eq!(sizes, vec![1000, 1000, 500]);
    let queries: Vec<String> = fake
        .requests()
        .iter()
        .map(|r| r.query().to_string())
        .collect();
    let head = "type=Vehicle&attrs=speed,location&limit=1000&q=speed%3E50";
    assert_eq!(
        queries,
        vec![
            format!("{head}&count=true"),
            format!("{head}&offset=1000"),
            format!("{head}&offset=2000"),
        ]
    );
    let r = &fake.requests()[0];
    assert_eq!(r.path(), "/api/endpoint/test/ngsi-ld/v1/entities");
    assert_eq!(r.method, "GET");
    assert_eq!(r.header("accept").as_deref(), Some("application/json"));
}

#[tokio::test]
async fn query_refuses_more_than_one_run_reads() {
    let fake = FakeBroker::start().await;
    fake.with(|s| s.entities = vehicles(60));
    let (sctx, _p, _stop) = ctx("org-q", true);
    let mut i = input(
        conf(
            &fake.base(),
            json!({"type": "Vehicle", "limit": 10, "max_entities": 50}),
        ),
        &sctx,
    )
    .expect("builds");
    let err = read(&mut i, &sctx)
        .await
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(
        err.contains("the source holds 60 entities, more than the 50 one run reads"),
        "{err}"
    );
}

#[tokio::test]
async fn query_refuses_a_full_page_without_a_count() {
    let fake = FakeBroker::start().await;
    fake.with(|s| {
        s.entities = vehicles(20);
        s.send_count = false;
    });
    let (sctx, _p, _stop) = ctx("org-q", true);
    let mut i = input(
        conf(&fake.base(), json!({"type": "Vehicle", "limit": 10})),
        &sctx,
    )
    .expect("builds");
    let err = read(&mut i, &sctx)
        .await
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(
        err.contains("did not say how many entities it holds"),
        "{err}"
    );

    // A short page needs no count.
    fake.with(|s| s.entities = vehicles(5));
    let mut i = input(
        conf(&fake.base(), json!({"type": "Vehicle", "limit": 10})),
        &sctx,
    )
    .expect("builds");
    let (batch, ack) = read(&mut i, &sctx).await.expect("one page");
    assert_eq!(batch.len(), 5);
    ack.ack();
    assert!(matches!(
        read(&mut i, &sctx).await,
        Err(CoreError::EndOfInput)
    ));
}

#[tokio::test]
async fn simplified_format_asks_for_key_values() {
    let fake = FakeBroker::start().await;
    fake.with(|s| s.entities = vec![json!({"id": "urn:a", "type": "T", "speed": 3})]);
    let (sctx, _p, _stop) = ctx("org-q", true);
    let mut i = input(
        conf(&fake.base(), json!({"type": "T", "format": "simplified"})),
        &sctx,
    )
    .expect("builds");
    let (batch, _ack) = read(&mut i, &sctx).await.expect("page");
    assert_eq!(ids(&batch), vec!["urn:a"]);
    assert!(fake.requests()[0].query().contains("options=keyValues"));
}

#[tokio::test]
async fn an_empty_source_ends_without_a_batch() {
    let fake = FakeBroker::start().await;
    let (sctx, _p, _stop) = ctx("org-q", true);
    let mut i = input(conf(&fake.base(), json!({"type": "T"})), &sctx).expect("builds");
    assert!(matches!(
        read(&mut i, &sctx).await,
        Err(CoreError::EndOfInput)
    ));
    assert_eq!(fake.requests().len(), 1);
}

#[tokio::test]
async fn changes_only_emits_again_only_what_changed_and_moves_on_ack() {
    let fake = FakeBroker::start().await;
    let stamp = |id: &str, at: &str| json!({"id": id, "type": "T", "modifiedAt": at});
    fake.with(|s| {
        s.entities = vec![
            stamp("urn:a", "2026-01-01T00:00:00Z"),
            stamp("urn:b", "2026-01-01T00:00:00Z"),
        ]
    });
    let (sctx, _p, _stop) = ctx("org-changes", true);
    let mut i = input(
        conf(
            &fake.base(),
            json!({"type": "T", "changes_only": true, "interval": "20ms"}),
        ),
        &sctx,
    )
    .expect("builds");
    assert!(fake.requests().is_empty());
    let (batch, ack) = read(&mut i, &sctx).await.expect("pass 1");
    assert_eq!(ids(&batch), vec!["urn:a", "urn:b"]);
    assert!(fake.requests()[0].query().contains("options=sysAttrs"));
    ack.nack(BatchError::new(batch, "the output refused it"));

    let (batch, ack) = read(&mut i, &sctx).await.expect("pass 2");
    assert_eq!(
        ids(&batch),
        vec!["urn:a", "urn:b"],
        "a nack keeps the gate where it was"
    );
    ack.ack();

    fake.with(|s| s.entities[0] = stamp("urn:a", "2026-01-02T00:00:00Z"));
    let (batch, ack) = read(&mut i, &sctx).await.expect("pass 3");
    assert_eq!(ids(&batch), vec!["urn:a"]);
    ack.ack();
}

#[tokio::test]
async fn temporal_moves_the_watermark_only_on_ack() {
    let fake = FakeBroker::start().await;
    let page = |at: &str| {
        json!([{"id": "urn:a", "type": "T", "speed": [{"type": "Property", "value": 1, "observedAt": at}]}]).to_string()
    };
    fake.script(200, &[], &page("2026-01-01T01:00:00Z"));
    let (sctx, _p, _stop) = ctx("org-t", true);
    let mut i = input(
        conf(
            &fake.base(),
            json!({"mode": "temporal", "type": "T", "time_at": "2026-01-01T00:00:00Z", "interval": "20ms"}),
        ),
        &sctx,
    )
    .expect("builds");
    let (batch, ack) = read(&mut i, &sctx).await.expect("pass 1");
    assert_eq!(batch.len(), 1);
    let first = fake.requests()[0].clone();
    assert_eq!(
        first.path(),
        "/api/endpoint/test/ngsi-ld/v1/temporal/entities"
    );
    assert!(
        first
            .query()
            .contains("timerel=after&timeAt=2026-01-01T00%3A00%3A00.000Z"),
        "{}",
        first.query()
    );

    // Not acked yet: the next pass still starts at the configured time.
    fake.script(200, &[], &page("2026-01-01T01:00:00Z"));
    let (_b, ack2) = read(&mut i, &sctx).await.expect("pass 2");
    let second = fake.requests().last().cloned().expect("request");
    assert!(
        second
            .query()
            .contains("timeAt=2026-01-01T00%3A00%3A00.000Z"),
        "{}",
        second.query()
    );
    ack.ack();
    ack2.ack();

    fake.script(200, &[], &page("2026-01-01T02:00:00Z"));
    let (_b, ack3) = read(&mut i, &sctx).await.expect("pass 3");
    let third = fake.requests().last().cloned().expect("request");
    assert!(
        third
            .query()
            .contains("timeAt=2026-01-01T01%3A00%3A00.000Z"),
        "{}",
        third.query()
    );
    ack3.ack();
}

#[tokio::test]
async fn temporal_follows_the_paging_link_on_the_same_endpoint() {
    let fake = FakeBroker::start().await;
    let next = format!(
        "<{}/temporal/entities?type=T&page=2>; rel=\"intervalafter\"",
        fake.base()
    );
    fake.script(
        200,
        &[("link", next.as_str())],
        r#"[{"id":"urn:a","type":"T"}]"#,
    );
    fake.script(200, &[], r#"[{"id":"urn:b","type":"T"}]"#);
    let (sctx, _p, _stop) = ctx("org-t", true);
    let mut i = input(
        conf(
            &fake.base(),
            json!({"mode": "temporal", "type": "T", "time_at": "2026-01-01T00:00:00Z"}),
        ),
        &sctx,
    )
    .expect("builds");
    let (b1, a1) = read(&mut i, &sctx).await.expect("page 1");
    let (b2, a2) = read(&mut i, &sctx).await.expect("page 2");
    assert_eq!(ids(&b1), vec!["urn:a"]);
    assert_eq!(ids(&b2), vec!["urn:b"]);
    a1.ack();
    a2.ack();
    assert!(matches!(
        read(&mut i, &sctx).await,
        Err(CoreError::EndOfInput)
    ));
    assert!(fake.requests()[1].query().contains("page=2"));
}

#[tokio::test]
async fn temporal_refuses_a_link_that_leaves_the_endpoint() {
    let fake = FakeBroker::start().await;
    fake.script(
        200,
        &[(
            "link",
            "<http://evil.example/ngsi-ld/v1/temporal/entities?x=1>; rel=\"intervalafter\"",
        )],
        r#"[{"id":"urn:a","type":"T"}]"#,
    );
    let (sctx, _p, _stop) = ctx("org-t", true);
    let mut i = input(
        conf(
            &fake.base(),
            json!({"mode": "temporal", "type": "T", "time_at": "2026-01-01T00:00:00Z"}),
        ),
        &sctx,
    )
    .expect("builds");
    let res = read(&mut i, &sctx).await;
    assert!(
        matches!(res, Err(CoreError::AccessDenied { .. })),
        "{:?}",
        res.err()
    );
    assert_eq!(fake.requests().len(), 1);
}

#[tokio::test]
async fn query_retries_then_fails_a_single_pass() {
    let fake = FakeBroker::start().await;
    for _ in 0..3 {
        fake.script(503, &[("retry-after", "0")], "busy");
    }
    let (sctx, _p, _stop) = ctx("org-q", true);
    let mut i = input(
        conf(&fake.base(), json!({"type": "T", "retries": 2})),
        &sctx,
    )
    .expect("builds");
    let res = read(&mut i, &sctx).await;
    assert!(res.is_err());
    assert_eq!(fake.requests().len(), 3);
}

#[tokio::test]
async fn read_configs_are_checked_at_build() {
    let fake = FakeBroker::start().await;
    let (sctx, _p, _stop) = ctx("org-q", true);
    let base = fake.base();
    for c in [
        conf(&base, json!({})),
        conf(&base, json!({"type": "T", "limit": 1001})),
        conf(&base, json!({"type": "T", "mode": "subscribe"})),
        conf(&base, json!({"type": "T", "bogus": 1})),
        conf(&base, json!({"type": "T", "changes_cap": 10})),
        conf(
            &base,
            json!({"type": "T", "time_at": "2026-01-01T00:00:00Z"}),
        ),
        conf(&base, json!({"mode": "temporal", "type": "T"})),
        conf(
            &base,
            json!({"mode": "temporal", "type": "T", "time_at": "2026-01-01T00:00:00Z", "timerel": "before", "interval": "1s"}),
        ),
        conf(
            &base,
            json!({"mode": "temporal", "type": "T", "time_at": "yesterday"}),
        ),
        conf(
            &base,
            json!({"type": "T", "headers": {"Authorization": "Basic x"}}),
        ),
    ] {
        assert!(input(c.clone(), &sctx).is_err(), "{c} must be refused");
    }
}

// --- Bento port: internal/httpclient/client_test.go ---

#[tokio::test]
async fn bento_client_test_http_client_receive() {
    let fake = FakeBroker::start().await;
    let (sctx, _p, _stop) = ctx("org-q", true);
    let mut i = input(
        conf(&fake.base(), json!({"type": "T", "interval": "1ms"})),
        &sctx,
    )
    .expect("builds");
    for j in 0..20 {
        let body = json!([{"id": format!("test{j}PART-A"), "type": "T"}]).to_string();
        fake.script(201, &[("foo-bar", "baz-0")], &body);
        let (batch, ack) = read(&mut i, &sctx).await.expect("page");
        assert_eq!(ids(&batch), vec![format!("test{j}PART-A")]);
        assert_eq!(
            batch[0].meta_get_str("foo-bar"),
            "",
            "answer headers never become metadata"
        );
        ack.ack();
    }
}
