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
async fn changes_only_emits_an_entity_without_id_on_every_pass() {
    let fake = FakeBroker::start().await;
    fake.with(|s| s.entities = vec![json!({"id": "urn:a", "type": "T"}), json!({"type": "T"})]);
    let (sctx, _p, _stop) = ctx("org-changes-noid", true);
    let mut i = input(
        conf(
            &fake.base(),
            json!({"type": "T", "changes_only": true, "interval": "20ms"}),
        ),
        &sctx,
    )
    .expect("builds");
    let (batch, ack) = read(&mut i, &sctx).await.expect("pass 1");
    assert_eq!(ids(&batch), vec!["urn:a", ""]);
    ack.ack();
    let (batch, _ack) = read(&mut i, &sctx).await.expect("pass 2");
    assert_eq!(
        ids(&batch),
        vec![""],
        "no id to remember it by: it comes again"
    );
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
async fn a_new_temporal_pass_forgets_the_page_the_output_refused() {
    let fake = FakeBroker::start().await;
    let page = |at: &str| {
        json!([{"id": "urn:a", "type": "T", "speed": [{"type": "Property", "value": 1, "observedAt": at}]}])
            .to_string()
    };
    fake.script(200, &[], &page("2026-02-01T00:00:00Z"));
    fake.script(200, &[], &page("2026-03-01T00:00:00Z"));
    fake.script(200, &[], &page("2026-03-01T00:00:00Z"));
    let (sctx, _p, _stop) = ctx("org-t", true);
    let mut i = input(
        conf(
            &fake.base(),
            json!({"mode": "temporal", "type": "T", "time_at": "2026-01-01T00:00:00Z", "interval": "20ms"}),
        ),
        &sctx,
    )
    .expect("builds");
    let (b, ack) = read(&mut i, &sctx).await.expect("pass 1");
    ack.nack(BatchError::new(b, "refused"));
    let (_b, ack) = read(&mut i, &sctx).await.expect("pass 2");
    ack.ack();
    let (_b, _ack) = read(&mut i, &sctx).await.expect("pass 3");
    let third = fake.requests()[2].query().to_string();
    assert!(
        third.contains("timeAt=2026-03-01T00%3A00%3A00.000Z"),
        "pass 2's ack moves the watermark: {third}"
    );
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
async fn temporal_ignores_a_paging_link_on_an_empty_page() {
    let fake = FakeBroker::start().await;
    let next = format!("<{}/temporal/entities?page=2>; rel=\"next\"", fake.base());
    fake.script(200, &[("link", next.as_str())], "[]");
    let (sctx, _p, _stop) = ctx("org-t", true);
    let mut i = input(
        conf(
            &fake.base(),
            json!({"mode": "temporal", "type": "T", "time_at": "2026-01-01T00:00:00Z"}),
        ),
        &sctx,
    )
    .expect("builds");
    assert!(matches!(
        read(&mut i, &sctx).await,
        Err(CoreError::EndOfInput)
    ));
    assert_eq!(fake.requests().len(), 1);
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

#[tokio::test]
async fn read_config_refusals_name_the_rule() {
    let fake = FakeBroker::start().await;
    let (sctx, _p, _stop) = ctx("org-q", true);
    let base = fake.base();
    let t0 = "2026-01-01T00:00:00Z";
    for (c, want) in [
        (
            json!({"mode": "push"}),
            "mode must be query, temporal or notification",
        ),
        (
            json!({"geo_q": {"geometry": "Point", "coordinates": "[1,2]"}}),
            "geo_q.georel is required",
        ),
        (
            json!({"geo_q": {"georel": "near", "coordinates": "[1,2]"}}),
            "geo_q.geometry is required",
        ),
        (
            json!({"geo_q": {"georel": "near", "geometry": "Point"}}),
            "geo_q.coordinates is required",
        ),
        (
            json!({"geo_q": {"georel": "near", "geometry": "Point", "coordinates": 1}}),
            "geo_q.coordinates is required",
        ),
        (
            json!({"geo_q": {"georel": "near", "geometry": "Point", "coordinates": ""}}),
            "geo_q.coordinates is required",
        ),
        (json!({"geo_q": {"zz": 1}}), "field zz not recognised"),
        (
            json!({"id_pattern": "urn:.*"}),
            "id_pattern needs type (or another selector) beside it",
        ),
        (
            json!({"format": "concise"}),
            "a query needs at least one of type, id, attrs, q or geo_q",
        ),
        (
            json!({"type": "T", "format": "flat"}),
            "format must be normalized, concise",
        ),
        (
            json!({"type": "T", "last_n": 1}),
            "field last_n belongs to mode temporal",
        ),
        (
            json!({"type": "T", "limit": 0}),
            "limit must be between 1 and 1000",
        ),
        (
            json!({"type": "T", "max_entities": 0}),
            "max_entities must be between 1 and",
        ),
        (
            json!({"type": "T", "max_entities": 1_000_001}),
            "max_entities must be between 1 and",
        ),
        (
            json!({"type": "T", "changes_only": true, "changes_cap": 0}),
            "changes_cap must be between 1 and",
        ),
        (
            json!({"type": "T", "changes_only": true, "changes_cache": "nope"}),
            "nope",
        ),
        (
            json!({"type": "T", "changes_cache": "c"}),
            "need changes_only: true",
        ),
        (json!({"type": "T", "interval": "soon"}), "field interval"),
        (
            json!({"mode": "temporal", "type": "T", "time_at": t0, "max_entities": 5}),
            "field max_entities belongs to mode query",
        ),
        (
            json!({"mode": "temporal", "time_at": t0}),
            "a temporal query needs type, id, attrs, q or geo_q",
        ),
        (
            json!({"mode": "temporal", "type": "T", "time_at": t0, "timerel": "during"}),
            "timerel must be after, before or between",
        ),
        (
            json!({"mode": "temporal", "type": "T", "time_at": t0, "timerel": "between"}),
            "timerel between needs end_time_at",
        ),
        (
            json!({"mode": "temporal", "type": "T", "time_at": t0, "end_time_at": "later"}),
            "end_time_at must be an RFC 3339 date-time",
        ),
        (
            json!({"mode": "temporal", "type": "T", "time_at": t0, "limit": 0}),
            "limit must be between 1 and 1000",
        ),
        (
            json!({"mode": "temporal", "type": "T", "time_at": t0, "last_n": 0}),
            "last_n must be at least 1",
        ),
        (
            json!({"mode": "temporal", "type": "T", "time_at": t0, "max_pages": 0}),
            "max_pages must be between 1 and",
        ),
        (
            json!({"mode": "temporal", "type": "T", "time_at": t0, "max_pages": 100_001}),
            "max_pages must be between 1 and",
        ),
        (
            json!({"mode": "temporal", "type": "T", "time_at": t0, "interval": "x"}),
            "field interval",
        ),
        (
            json!({"mode": "temporal", "type": "T", "time_at": t0, "format": "flat"}),
            "format must be",
        ),
    ] {
        let e = input(conf(&base, c.clone()), &sctx)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(e.contains(want), "{c}: {e}");
    }
    for mode in ["query", "temporal"] {
        let e = input(json!({"mode": mode, "type": "T", "time_at": t0}), &sctx)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(e.contains("field url is required"), "{mode}: {e}");
    }
}

#[tokio::test]
async fn read_limits_take_their_ceilings() {
    let fake = FakeBroker::start().await;
    let (sctx, _p, _stop) = ctx("org-q", true);
    for c in [
        json!({"type": "T", "limit": 1000, "max_entities": 1_000_000}),
        json!({"type": "T", "changes_only": true, "changes_cap": 1_000_000}),
        json!({"mode": "temporal", "type": "T", "time_at": "2026-01-01T00:00:00Z",
            "limit": 1000, "max_pages": 100_000}),
    ] {
        assert!(input(conf(&fake.base(), c.clone()), &sctx).is_ok(), "{c}");
    }
}

#[tokio::test]
async fn a_pass_ends_on_a_short_page_or_at_the_count_without_an_extra_request() {
    let fake = FakeBroker::start().await;
    let (sctx, _p, _stop) = ctx("org-q", true);
    fake.with(|s| {
        s.entities = vehicles(3);
        s.send_count = false;
    });
    let mut i = input(
        conf(&fake.base(), json!({"type": "Vehicle", "limit": 10})),
        &sctx,
    )
    .expect("builds");
    let (b, _a) = read(&mut i, &sctx).await.expect("short page");
    assert_eq!(b.len(), 3);
    assert!(matches!(
        read(&mut i, &sctx).await,
        Err(CoreError::EndOfInput)
    ));
    assert_eq!(fake.requests().len(), 1, "a short page ends the pass");

    fake.with(|s| {
        s.entities = vehicles(4);
        s.send_count = true;
    });
    let mut i = input(
        conf(
            &fake.base(),
            json!({"type": "Vehicle", "limit": 2, "max_entities": 4}),
        ),
        &sctx,
    )
    .expect("builds");
    for _ in 0..2 {
        let (b, _a) = read(&mut i, &sctx).await.expect("full page");
        assert_eq!(b.len(), 2);
    }
    assert!(matches!(
        read(&mut i, &sctx).await,
        Err(CoreError::EndOfInput)
    ));
    assert_eq!(
        fake.requests().len(),
        3,
        "the count ends the pass: no empty page asked"
    );
}

#[tokio::test]
async fn the_next_pass_waits_its_interval() {
    let fake = FakeBroker::start().await;
    fake.with(|s| s.entities = vehicles(1));
    let (sctx, _p, _stop) = ctx("org-q", true);
    for c in [
        json!({"type": "Vehicle", "interval": "300ms"}),
        json!({"mode": "temporal", "type": "Vehicle", "time_at": "2026-01-01T00:00:00Z", "interval": "300ms"}),
    ] {
        let mut i = input(conf(&fake.base(), c.clone()), &sctx).expect("builds");
        for _ in 0..2 {
            fake.script(200, &[], r#"[{"id":"urn:t","type":"Vehicle"}]"#);
        }
        let started = std::time::Instant::now();
        let (_b, ack) = read(&mut i, &sctx).await.expect("pass 1");
        ack.ack();
        let (_b, _ack) = read(&mut i, &sctx).await.expect("pass 2");
        assert!(
            started.elapsed() >= Duration::from_millis(250),
            "{c}: the second pass came after {:?}",
            started.elapsed()
        );
    }
}

#[tokio::test]
async fn the_query_url_carries_every_selector_in_the_portal_order() {
    let fake = FakeBroker::start().await;
    let (sctx, _p, _stop) = ctx("org-q", true);
    let mut i = input(
        conf(
            &fake.base(),
            json!({"id": "urn:a,urn:b", "id_pattern": "urn:.*", "scope_q": "/city/#",
                "format": "concise", "geo_q": {"georel": "near;maxDistance==500",
                "geometry": "Point", "coordinates": [24.9, 60.1], "geoproperty": "loc"}}),
        ),
        &sctx,
    )
    .expect("builds");
    assert!(matches!(
        read(&mut i, &sctx).await,
        Err(CoreError::EndOfInput)
    ));
    let q = fake.requests()[0].query().to_string();
    assert_eq!(
        q,
        "id=urn%3Aa,urn%3Ab&idPattern=urn%3A.%2A&options=concise&limit=1000&scopeQ=%2Fcity%2F%23\
         &georel=near%3BmaxDistance%3D%3D500&geometry=Point&coordinates=%5B24.9%2C60.1%5D\
         &geoproperty=loc&count=true"
    );

    let mut i = input(
        conf(
            &fake.base(),
            json!({"geo_q": {"georel": "within", "geometry": "Polygon", "coordinates": "[[[0,0]]]"}}),
        ),
        &sctx,
    )
    .expect("a geo-query alone selects");
    assert!(matches!(
        read(&mut i, &sctx).await,
        Err(CoreError::EndOfInput)
    ));
    assert!(fake.requests()[1]
        .query()
        .starts_with("limit=1000&georel=within"));
}

#[tokio::test]
async fn query_answers_that_are_no_entity_array_fail_the_pass() {
    let fake = FakeBroker::start().await;
    let (sctx, _p, _stop) = ctx("org-q", true);
    for (body, want) in [
        ("not json", "the broker answered a body that is not JSON"),
        (
            r#"{"id":"urn:a"}"#,
            "where an array of entities is expected",
        ),
        (r#"[{"id":"urn:a"}, 3]"#, "item 1 of the answer is a"),
    ] {
        fake.script(200, &[], body);
        let mut i = input(conf(&fake.base(), json!({"type": "T"})), &sctx).expect("builds");
        let e = read(&mut i, &sctx)
            .await
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(e.contains(want), "{body}: {e}");
    }
    fake.script(200, &[], "not json");
    let mut t = input(
        conf(
            &fake.base(),
            json!({"mode": "temporal", "type": "T", "time_at": "2026-01-01T00:00:00Z"}),
        ),
        &sctx,
    )
    .expect("builds");
    let e = read(&mut t, &sctx)
        .await
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(
        e.contains("the broker answered a body that is not JSON"),
        "{e}"
    );
}

#[tokio::test]
async fn a_stopped_stream_ends_both_read_modes() {
    let fake = FakeBroker::start().await;
    let (sctx, _p, stop) = ctx("org-q", true);
    stop.send(true).expect("stop");
    for c in [
        json!({"type": "T"}),
        json!({"mode": "temporal", "type": "T", "time_at": "2026-01-01T00:00:00Z"}),
    ] {
        let mut i = input(conf(&fake.base(), c), &sctx).expect("builds");
        assert!(matches!(
            read(&mut i, &sctx).await,
            Err(CoreError::EndOfInput)
        ));
    }
    assert!(fake.requests().is_empty());
}

#[tokio::test]
async fn with_an_interval_a_failed_pass_waits_and_the_next_one_reads() {
    let fake = FakeBroker::start().await;
    fake.with(|s| s.entities = vehicles(1));
    fake.script(500, &[], "down");
    let (sctx, _p, _stop) = ctx("org-q", true);
    let mut i = input(
        conf(
            &fake.base(),
            json!({"type": "Vehicle", "retries": 0, "interval": "30ms"}),
        ),
        &sctx,
    )
    .expect("builds");
    let (batch, _ack) = read(&mut i, &sctx).await.expect("the second pass reads");
    assert_eq!(ids(&batch), vec!["urn:ngsi-ld:Vehicle:0"]);
    assert_eq!(fake.requests().len(), 2);
    assert_eq!(
        sctx.metrics
            .get_counter(&ekiben_io_ngsild::metrics::read_key("read")),
        1
    );

    fake.script(500, &[], "down");
    let mut t = input(
        conf(
            &fake.base(),
            json!({"mode": "temporal", "type": "T", "time_at": "2026-01-01T00:00:00Z",
                "retries": 0, "interval": "30ms"}),
        ),
        &sctx,
    )
    .expect("builds");
    fake.script(200, &[], r#"[{"id":"urn:t","type":"T"}]"#);
    let (batch, _ack) = read(&mut t, &sctx).await.expect("the second pass reads");
    assert_eq!(ids(&batch), vec!["urn:t"]);
}

#[tokio::test]
async fn a_stop_during_the_interval_wait_ends_the_read() {
    let fake = FakeBroker::start().await;
    for c in [
        json!({"type": "T", "interval": "60s"}),
        json!({"mode": "temporal", "type": "T", "time_at": "2026-01-01T00:00:00Z", "interval": "60s"}),
    ] {
        let (sctx, _p, stop) = ctx("org-q", true);
        let mut i = input(conf(&fake.base(), c), &sctx).expect("builds");
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let _ = stop.send(true);
        });
        // The source is empty: the first pass ends at once and the next waits its interval.
        assert!(matches!(
            read(&mut i, &sctx).await,
            Err(CoreError::EndOfInput)
        ));
    }
}

/// A cache that refuses every write.
struct Refusing;

#[async_trait::async_trait]
impl ekiben_core::Cache for Refusing {
    async fn get(&self, key: &str) -> Result<bytes::Bytes, CoreError> {
        Err(CoreError::KeyNotFound(key.to_string()))
    }
    async fn set(&self, _: &str, _: bytes::Bytes, _: Option<Duration>) -> Result<(), CoreError> {
        Err(CoreError::other("full"))
    }
    async fn add(&self, _: &str, _: bytes::Bytes, _: Option<Duration>) -> Result<(), CoreError> {
        Err(CoreError::other("full"))
    }
    async fn delete(&self, _: &str) -> Result<(), CoreError> {
        Ok(())
    }
}

#[tokio::test]
async fn a_cache_refusing_the_write_is_logged_and_the_entity_comes_again() {
    let fake = FakeBroker::start().await;
    fake.with(|s| {
        s.entities = vec![
            json!({"type": "T", "name": "no id"}),
            json!({"id": "urn:a", "type": "T"}),
        ]
    });
    let (sctx, _p, _stop) = ctx("org-cache-full", true);
    sctx.resources
        .insert_cache(&sctx.org.org_id, "full", std::sync::Arc::new(Refusing));
    let c = conf(
        &fake.base(),
        json!({"type": "T", "changes_only": true, "changes_cache": "full"}),
    );
    let (logs, _guard) = common::capture_logs();
    let mut i = input(c.clone(), &sctx).expect("builds");
    let (batch, ack) = read(&mut i, &sctx).await.expect("first read");
    assert_eq!(
        ids(&batch),
        vec!["", "urn:a"],
        "an entity without id passes the gate"
    );
    ack.ack();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        logs.text()
            .contains("ngsi_ld changes_cache refused an entry"),
        "{}",
        logs.text()
    );
    let mut again = input(c, &sctx).expect("builds");
    let (batch, _ack) = read(&mut again, &sctx).await.expect("read again");
    assert_eq!(ids(&batch), vec!["", "urn:a"]);
}

#[tokio::test]
async fn changes_only_with_a_cache_resource_remembers_across_inputs() {
    let fake = FakeBroker::start().await;
    fake.with(|s| s.entities = vec![json!({"id": "urn:a", "type": "T"})]);
    let (sctx, _p, _stop) = ctx("org-cache", true);
    sctx.resources.insert_cache(
        &sctx.org.org_id,
        "seen",
        std::sync::Arc::new(ekiben_processors::resources::MemoryCache::new(None, None)),
    );
    let c = conf(
        &fake.base(),
        json!({"type": "T", "changes_only": true, "changes_cache": "seen"}),
    );
    let mut i = input(c.clone(), &sctx).expect("builds");
    let (batch, ack) = read(&mut i, &sctx).await.expect("first read");
    assert_eq!(ids(&batch), vec!["urn:a"]);
    ack.ack();
    // The cache write runs on the runtime after the ack.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut again = input(c, &sctx).expect("builds");
    assert!(
        matches!(read(&mut again, &sctx).await, Err(CoreError::EndOfInput)),
        "an unchanged entity is not emitted again by a new input sharing the cache"
    );
}

#[tokio::test]
async fn temporal_urls_carry_between_last_n_and_before() {
    let fake = FakeBroker::start().await;
    let (sctx, _p, _stop) = ctx("org-t", true);
    for (c, want) in [
        (
            json!({"mode": "temporal", "attrs": "speed", "timerel": "between",
                "time_at": "2026-01-01T00:00:00Z", "end_time_at": "2026-01-02T00:00:00Z", "last_n": 5}),
            "attrs=speed&limit=100&lastN=5&timerel=between&timeAt=2026-01-01T00%3A00%3A00.000Z\
             &endTimeAt=2026-01-02T00%3A00%3A00.000Z",
        ),
        (
            json!({"mode": "temporal", "q": "speed>1", "timerel": "before",
                "time_at": "2026-01-01T00:00:00Z", "format": "simplified"}),
            "options=keyValues&limit=100&q=speed%3E1&timerel=before&timeAt=2026-01-01T00%3A00%3A00.000Z",
        ),
    ] {
        let mut i = input(conf(&fake.base(), c), &sctx).expect("builds");
        assert!(matches!(read(&mut i, &sctx).await, Err(CoreError::EndOfInput)));
        let last = fake.requests().pop().expect("one request");
        assert_eq!(last.query(), want);
    }
}

#[tokio::test]
async fn temporal_refuses_a_bad_paging_link_and_one_past_max_pages() {
    let fake = FakeBroker::start().await;
    let (sctx, _p, _stop) = ctx("org-t", true);
    let c = |extra: JsonValue| {
        let mut v = json!({"mode": "temporal", "type": "T", "time_at": "2026-01-01T00:00:00Z"});
        if let (Some(m), JsonValue::Object(e)) = (v.as_object_mut(), extra) {
            m.extend(e);
        }
        conf(&fake.base(), v)
    };
    fake.script(
        200,
        &[("link", "<http://[bad>; rel=\"next\"")],
        r#"[{"id":"urn:a","type":"T"}]"#,
    );
    let mut i = input(c(json!({})), &sctx).expect("builds");
    let e = read(&mut i, &sctx)
        .await
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(e.contains("the broker's paging link is no URL"), "{e}");

    let next = format!("<{}/temporal/entities?page=2>; rel=\"next\"", fake.base());
    fake.script(
        200,
        &[("link", next.as_str())],
        r#"[{"id":"urn:a","type":"T"}]"#,
    );
    let mut i = input(c(json!({"max_pages": 1})), &sctx).expect("builds");
    let e = read(&mut i, &sctx)
        .await
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(e.contains("runs past max_pages (1)"), "{e}");
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
