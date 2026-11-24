// SPDX-License-Identifier: EUPL-1.2
//! Live-NATS integration (env-gated, same pattern as the live-PG
//! tests): the broadcast-vs-balanced assertion, the claim check on
//! the wire, and Nats-Msg-Id dedup. Skips loudly without
//! ANTARES_TEST_NATS_URL (CI runs a nats:2 -js service).

use antares_bus::nats::NatsBus;
use antares_bus::{ChangeEvent, ChangeOp};
use antares_model::{EntityId, TenantId};
use futures_util::StreamExt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

macro_rules! require_nats {
    () => {
        match std::env::var("ANTARES_TEST_NATS_URL") {
            Ok(url) => url,
            Err(_) => {
                eprintln!("SKIP: ANTARES_TEST_NATS_URL not set");
                return;
            }
        }
    };
}

fn nonce() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos()
}

fn event(tenant: &TenantId, id: &str, seq: i64) -> ChangeEvent {
    ChangeEvent {
        tenant: tenant.clone(),
        entity_id: EntityId::new(id).expect("id"),
        types: vec!["https://x/T".into()],
        op: ChangeOp::Create,
        changed_attrs: vec![],
        payload: Some(serde_json::json!({"id": id, "type": ["https://x/T"]})),
        prev_payload: None,
        version: 1,
        incarnation: "2026-08-05T00:00:00Z".into(),
        seq,
        payload_ref: None,
        prev_payload_ref: None,
    }
}

/// Two instances on the SAME durable split the work — each message is
/// delivered to exactly one of them; two ephemeral registry consumers each
/// see EVERY delta. The distinction that Scorpio's `$[quarkus.uuid}` typo
/// silently destroyed, asserted against a live server.
#[tokio::test(flavor = "multi_thread")]
async fn balanced_splits_and_broadcast_duplicates() {
    let url = require_nats!();
    let bus = NatsBus::connect(&url).await.expect("connect");
    let n = nonce();
    let tenant = TenantId::new(&format!("bal{n}")).expect("tenant");

    // two consumers, ONE durable — a shared work queue
    let durable = format!("bal-{n}");
    let c1 = bus.consume_balanced(&durable).await.expect("consumer 1");
    let c2 = bus.consume_balanced(&durable).await.expect("consumer 2");

    const N: usize = 8;
    for i in 0..N {
        bus.publish(&event(&tenant, &format!("urn:bal:{n}:{i}"), i as i64 + 1))
            .await
            .expect("publish");
    }

    // both instances pull; the union must be exactly the N messages, no
    // message seen twice (each counted by its unique entity id)
    let mut seen = std::collections::HashSet::new();
    let mut total = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut s1 = c1.messages().await.expect("stream 1");
    let mut s2 = c2.messages().await.expect("stream 2");
    while total < N && tokio::time::Instant::now() < deadline {
        tokio::select! {
            Some(Ok(m)) = s1.next() => {
                if let Some(ev) = antares_bus::nats::decode(&m) {
                    if ev.tenant.as_str() == tenant.as_str() {
                        assert!(seen.insert(ev.entity_id.as_str().to_owned()),
                            "balanced durable delivered {} twice", ev.entity_id.as_str());
                        total += 1;
                    }
                }
                m.ack().await.expect("ack");
            }
            Some(Ok(m)) = s2.next() => {
                if let Some(ev) = antares_bus::nats::decode(&m) {
                    if ev.tenant.as_str() == tenant.as_str() {
                        assert!(seen.insert(ev.entity_id.as_str().to_owned()),
                            "balanced durable delivered {} twice", ev.entity_id.as_str());
                        total += 1;
                    }
                }
                m.ack().await.expect("ack");
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }
    assert_eq!(total, N, "balanced durable must deliver every message once");

    // broadcast: two EPHEMERAL registry consumers each see the same delta
    let b1 = bus.consume_registry_broadcast().await.expect("bc 1");
    let b2 = bus.consume_registry_broadcast().await.expect("bc 2");
    let delta =
        serde_json::json!({"tenant": tenant.as_str(), "id": format!("urn:reg:{n}"), "doc": {}});
    bus.publish_registry(tenant.as_str(), &delta)
        .await
        .expect("registry publish");
    for (label, c) in [("bc1", b1), ("bc2", b2)] {
        let mut s = c.messages().await.expect("stream");
        let got = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let Some(Ok(m)) = s.next().await else {
                    continue;
                };
                let v: serde_json::Value = serde_json::from_slice(&m.payload).expect("json");
                m.ack().await.expect("ack");
                if v["tenant"] == tenant.as_str() {
                    return v;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{label} never saw the broadcast delta"));
        assert_eq!(got["id"], format!("urn:reg:{n}"));
    }
}

/// A payload over the claim-check threshold travels as a reference, and
/// the whole message stays far under NATS's ~1 MB cap.
#[tokio::test(flavor = "multi_thread")]
async fn claim_check_strips_oversized_payloads_on_the_wire() {
    let url = require_nats!();
    let bus = NatsBus::connect(&url).await.expect("connect");
    let n = nonce();
    let tenant = TenantId::new(&format!("cc{n}")).expect("tenant");
    let durable = format!("cc-{n}");
    let c = bus.consume_balanced(&durable).await.expect("consumer");

    let mut ev = event(&tenant, &format!("urn:cc:{n}"), 1);
    ev.payload = Some(serde_json::Value::String(
        "x".repeat(antares_bus::CLAIM_CHECK_BYTES + 1),
    ));
    bus.publish(&ev).await.expect("publish");

    let mut s = c.messages().await.expect("stream");
    let got = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let Some(Ok(m)) = s.next().await else {
                continue;
            };
            let ev = antares_bus::nats::decode(&m);
            m.ack().await.expect("ack");
            if let Some(ev) = ev {
                if ev.tenant.as_str() == tenant.as_str() {
                    return ev;
                }
            }
        }
    })
    .await
    .expect("claim-checked event never arrived");
    assert!(
        got.payload.is_none(),
        "oversized body must not travel inline"
    );
    let r = got.payload_ref.expect("reference set");
    assert_eq!(r.entity_id.as_str(), format!("urn:cc:{n}"));
    assert_eq!(r.version, 1);
}

/// A drain retry republishing the same outbox seq is absorbed by the
/// stream's duplicate window — one delivery, not two.
#[tokio::test(flavor = "multi_thread")]
async fn msg_id_dedup_absorbs_republish() {
    let url = require_nats!();
    let bus = NatsBus::connect(&url).await.expect("connect");
    let n = nonce();
    let tenant = TenantId::new(&format!("dd{n}")).expect("tenant");
    let durable = format!("dd-{n}");
    let c = bus.consume_balanced(&durable).await.expect("consumer");

    let ev = event(&tenant, &format!("urn:dd:{n}"), 7);
    bus.publish(&ev).await.expect("publish 1");
    bus.publish(&ev)
        .await
        .expect("publish 2 (same Nats-Msg-Id)");

    let mut s = c.messages().await.expect("stream");
    let mut count = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    while tokio::time::Instant::now() < deadline {
        tokio::select! {
            Some(Ok(m)) = s.next() => {
                if let Some(got) = antares_bus::nats::decode(&m) {
                    if got.tenant.as_str() == tenant.as_str() {
                        count += 1;
                    }
                }
                m.ack().await.expect("ack");
            }
            _ = tokio::time::sleep(Duration::from_millis(200)) => {}
        }
    }
    assert_eq!(
        count, 1,
        "duplicate-window dedup must swallow the republish"
    );
}

/// The recovery contract the registry mirror depends on.
///
/// `consume_registry_broadcast` builds an EPHEMERAL consumer, which a NATS
/// restart or an inactivity gap deletes server-side. When that happens the
/// message stream must END rather than hang, because that is the signal the
/// broker's watcher loop reopens and re-hydrates on; a stream that blocks
/// forever would freeze this pod's registration mirror — and with it all
/// federation matching — while /q/health still reported the bus connected.
/// The second half asserts the other side of the contract: a freshly built
/// consumer really does receive what is published after the gap.
#[tokio::test(flavor = "multi_thread")]
async fn deleted_broadcast_consumer_ends_its_stream_and_a_fresh_one_resumes() {
    let url = require_nats!();
    let bus = NatsBus::connect(&url).await.expect("connect");
    let n = nonce();
    let tenant = format!("reopen{n}");

    let consumer = bus
        .consume_registry_broadcast()
        .await
        .expect("broadcast consumer");
    let name = consumer.cached_info().name.clone();
    let mut msgs = antares_bus::nats::messages(&consumer)
        .await
        .expect("stream");

    bus.publish_registry(&tenant, &serde_json::json!({"id": "urn:reg:before"}))
        .await
        .expect("publish before");
    let got = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(d) = antares_bus::nats::next_delta(&mut msgs).await {
                if d["id"] == "urn:reg:before" {
                    return d;
                }
            } else {
                panic!("stream ended before the first delta arrived");
            }
        }
    })
    .await
    .expect("first delta within 10s");
    assert_eq!(got["id"], "urn:reg:before");

    // The gap: delete the consumer exactly as a server restart would.
    bus.delete_registry_consumer(&name)
        .await
        .expect("delete consumer");

    // Contract: the stream terminates. A hang here is the freeze bug.
    let ended = tokio::time::timeout(
        Duration::from_secs(15),
        antares_bus::nats::next_delta(&mut msgs),
    )
    .await
    .expect("stream must end within 15s after its consumer is deleted, not hang");
    assert!(
        ended.is_none(),
        "a deleted consumer must end the stream so the watcher reopens"
    );

    // Recovery: a new consumer sees traffic published after the gap.
    let fresh = bus
        .consume_registry_broadcast()
        .await
        .expect("fresh consumer");
    let mut fresh_msgs = antares_bus::nats::messages(&fresh).await.expect("stream");
    bus.publish_registry(&tenant, &serde_json::json!({"id": "urn:reg:after"}))
        .await
        .expect("publish after");
    let got = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(d) = antares_bus::nats::next_delta(&mut fresh_msgs).await {
                if d["id"] == "urn:reg:after" {
                    return d;
                }
            } else {
                panic!("fresh stream ended instead of delivering");
            }
        }
    })
    .await
    .expect("delta after recovery within 10s");
    assert_eq!(got["id"], "urn:reg:after");
}

/// The subscription mirror's other half: the KV watch is reopenable, and a
/// reopened watch sees writes made while nothing was watching. The broker's
/// watcher reopens on end-of-watch and re-reads the bucket, so a write that
/// lands in the gap must not be lost to a pod that reconnects.
#[tokio::test(flavor = "multi_thread")]
async fn subscription_kv_survives_a_watch_gap() {
    let url = require_nats!();
    let bus = NatsBus::connect(&url).await.expect("connect");
    let kv = bus.subs_kv().await.expect("kv bucket");
    let n = nonce();
    let key = format!("sub.gap.{n}");

    let watch = kv.watch(&key).await.expect("watch");
    kv.put(
        &key,
        serde_json::to_vec(&serde_json::json!({"v": 1}))
            .expect("encode")
            .into(),
    )
    .await
    .expect("put v1");
    drop(watch);

    // Written with nobody watching — the gap.
    kv.put(
        &key,
        serde_json::to_vec(&serde_json::json!({"v": 2}))
            .expect("encode")
            .into(),
    )
    .await
    .expect("put v2");

    // A reopened watch replays current state, so the gap write is not lost.
    let mut reopened = kv.watch_with_history(&key).await.expect("reopen watch");
    let seen = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(Ok(entry)) = reopened.next().await {
            let doc: serde_json::Value = serde_json::from_slice(&entry.value).expect("decode");
            if doc["v"] == 2 {
                return doc;
            }
        }
        panic!("watch ended without replaying the gap write");
    })
    .await
    .expect("reopened watch replays within 10s");
    assert_eq!(seen["v"], 2, "a reopened watch must see the gap write");
}

/// Core request/reply (the WebSocket relay's channel): a request reaches
/// the pod replying on its subject and carries the reply back; with no
/// replier, after the replier is aborted, or once it declines, the request
/// is `None` instead of hanging.
#[tokio::test(flavor = "multi_thread")]
async fn core_request_reaches_the_replier_and_is_none_without_one() {
    let url = require_nats!();
    let holder = NatsBus::connect(&url).await.expect("holder");
    let asker = NatsBus::connect(&url).await.expect("asker");
    let subject = format!("antares.ws.test{}", nonce());
    let within = Duration::from_secs(2);
    assert_eq!(
        asker.request(subject.clone(), b"x".to_vec(), within).await,
        None
    );

    let task = holder.reply(subject.clone(), |p| {
        (p != b"stop").then(|| [b"seen:".as_slice(), p].concat())
    });
    let mut answered = None;
    for _ in 0..50 {
        answered = asker
            .request(subject.clone(), b"hello".to_vec(), within)
            .await;
        if answered.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(answered.as_deref(), Some(b"seen:hello".as_slice()));

    assert_eq!(
        asker
            .request(subject.clone(), b"stop".to_vec(), within)
            .await,
        None
    );
    task.await.expect("the replier ends when it declines");

    let task = holder.reply(subject.clone(), |p| Some(p.to_vec()));
    tokio::time::sleep(Duration::from_millis(200)).await;
    task.abort();
    let _aborted = task.await;
    assert_eq!(asker.request(subject, b"late".to_vec(), within).await, None);
}

/// `reply_ready` (T-7507, the MCP relay): a request sent the moment it
/// resolves is answered, with no wait for the subscription.
#[tokio::test(flavor = "multi_thread")]
async fn a_ready_replier_hears_the_first_request() {
    let url = require_nats!();
    let holder = NatsBus::connect(&url).await.expect("holder");
    let asker = NatsBus::connect(&url).await.expect("asker");
    for i in 0..20 {
        let subject = format!("antares.mcp.test{}.s{i}", nonce());
        let task = holder
            .reply_ready(subject.clone(), |p| Some(p.to_vec()))
            .await
            .expect("ready");
        let within = Duration::from_secs(2);
        let heard = asker.request(subject, b"first".to_vec(), within).await;
        assert_eq!(heard.as_deref(), Some(b"first".as_slice()), "try {i}");
        task.abort();
    }
}

/// A replier skips a message nobody waits on (no reply subject) and still
/// answers the next request; a subject the server refuses is refused at once.
#[tokio::test(flavor = "multi_thread")]
async fn a_replier_skips_unanswerable_messages_and_bad_subjects_are_refused() {
    let url = require_nats!();
    let holder = NatsBus::connect(&url).await.expect("holder");
    let subject = format!("antares.mcp.skip{}", nonce());
    let task = holder
        .reply_ready(subject.clone(), |p| Some(p.to_vec()))
        .await
        .expect("ready");
    let raw = async_nats::connect(&url).await.expect("raw client");
    raw.publish(subject.clone(), "no one waits".into())
        .await
        .expect("publish");
    raw.flush().await.expect("flush");
    let heard = holder
        .request(subject, b"asked".to_vec(), Duration::from_secs(2))
        .await;
    assert_eq!(heard.as_deref(), Some(b"asked".as_slice()));
    task.abort();

    let bad = "antares.mcp.bad subject".to_string();
    let e = holder
        .reply_ready(bad.clone(), |p| Some(p.to_vec()))
        .await
        .expect_err("a subject with a space is refused");
    assert!(e.to_string().starts_with("bus error: "), "{e}");
    // The fire-and-forget form logs the refusal and its task ends.
    tokio::time::timeout(
        Duration::from_secs(2),
        holder.reply(bad, |p| Some(p.to_vec())),
    )
    .await
    .expect("the reply task ends")
    .expect("joins");
}

/// The startup topology check accepts a shared durable and refuses a missing
/// or non-durable consumer by name; an event whose subject names another
/// tenant than its body is dropped by `decode`.
#[tokio::test(flavor = "multi_thread")]
async fn topology_and_tenant_checks_refuse_what_does_not_match() {
    let url = require_nats!();
    let bus = NatsBus::connect(&url).await.expect("connect");
    assert!(bus.connected());
    assert_eq!(bus.reconnects(), 0, "the first connect is no reconnect");
    let n = nonce();
    let durable = format!("topo-{n}");
    let consumer = bus.consume_balanced(&durable).await.expect("durable");
    let config = &consumer.cached_info().config;
    assert_eq!(
        config.ack_policy,
        async_nats::jetstream::consumer::AckPolicy::Explicit
    );
    assert_eq!(config.max_ack_pending, antares_bus::nats::MAX_ACK_PENDING);
    bus.assert_topology(&[durable.as_str()])
        .await
        .expect("present");
    let missing = format!("topo-missing-{n}");
    let e = bus
        .assert_topology(&[durable.as_str(), missing.as_str()])
        .await
        .expect_err("missing");
    assert!(
        e.to_string().contains(&format!(
            "balanced durable '{missing}' missing on ANTARES_CHANGES"
        )),
        "{e}"
    );
    let js = async_nats::jetstream::new(async_nats::connect(&url).await.expect("raw client"));
    let stream = js.get_stream("ANTARES_CHANGES").await.expect("stream");
    let named = format!("topo-eph-{n}");
    stream
        .create_consumer(async_nats::jetstream::consumer::pull::Config {
            name: Some(named.clone()),
            durable_name: None,
            inactive_threshold: Duration::from_secs(30),
            ..Default::default()
        })
        .await
        .expect("named ephemeral");
    let e = bus
        .assert_topology(&[named.as_str()])
        .await
        .expect_err("not durable");
    assert!(e.to_string().contains("is not durable"), "{e}");

    let a = TenantId::new(&format!("topa{n}")).expect("tenant");
    let b = TenantId::new(&format!("topb{n}")).expect("tenant");
    let id = format!("urn:topo:{n}");
    let body = serde_json::to_vec(&event(&b, &id, 1)).expect("json");
    let subject = antares_bus::subjects::change_subject(&a, "https://x/T", &id);
    js.publish(subject, body.into())
        .await
        .expect("publish")
        .await
        .expect("stored");
    let mut msgs = consumer.messages().await.expect("messages");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut dropped = false;
    while !dropped && tokio::time::Instant::now() < deadline {
        let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_secs(1), msgs.next()).await
        else {
            continue;
        };
        if std::str::from_utf8(&m.payload).is_ok_and(|p| p.contains(&id)) {
            // A subscriber, so the drop's log line is written as in production.
            let log = tracing_subscriber::fmt().with_test_writer().finish();
            let _log = tracing::subscriber::set_default(log);
            assert!(
                antares_bus::nats::decode(&m).is_none(),
                "a mismatched tenant is dropped"
            );
            dropped = true;
        }
        m.ack().await.expect("ack");
    }
    assert!(dropped, "the mismatched event was delivered to the durable");
}

/// `next_delta` acks and skips a registry message that is not JSON and
/// returns the next one that is (other tests' deltas on the broadcast are passed over).
#[tokio::test(flavor = "multi_thread")]
async fn next_delta_skips_a_message_that_is_not_json() {
    let url = require_nats!();
    let bus = NatsBus::connect(&url).await.expect("connect");
    let consumer = bus.consume_registry_broadcast().await.expect("consumer");
    let config = &consumer.cached_info().config;
    assert_eq!(
        config.durable_name, None,
        "ephemeral: every instance sees every delta"
    );
    assert_eq!(
        config.deliver_policy,
        async_nats::jetstream::consumer::DeliverPolicy::New
    );
    assert_eq!(
        config.ack_policy,
        async_nats::jetstream::consumer::AckPolicy::Explicit
    );
    assert_eq!(config.max_ack_pending, antares_bus::nats::MAX_ACK_PENDING);
    let mut stream = consumer.messages().await.expect("messages");
    let n = nonce();
    let tenant = TenantId::new(&format!("nd{n}")).expect("tenant");
    let subject = antares_bus::subjects::registry_subject(&tenant);
    let js = async_nats::jetstream::new(async_nats::connect(&url).await.expect("raw client"));
    let want = serde_json::json!({ "delta": n.to_string() });
    for body in ["not json".to_string(), want.to_string()] {
        js.publish(subject.clone(), body.into())
            .await
            .expect("publish")
            .await
            .expect("stored");
    }
    let found = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match antares_bus::nats::next_delta(&mut stream).await {
                Some(v) if v == want => break true,
                Some(_) => continue,
                None => break false,
            }
        }
    })
    .await
    .expect("a delta within 10 s");
    assert!(found, "the stream ended before the JSON delta");
}

/// A TCP hop between the bus and NATS that the test can cut and hold shut.
struct Hop {
    port: u16,
    open: std::sync::Arc<std::sync::atomic::AtomicBool>,
    links: std::sync::Arc<std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

impl Hop {
    async fn to(url: &str) -> Hop {
        let upstream = url.trim_start_matches("nats://").to_string();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let open = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let links = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (o, l) = (open.clone(), links.clone());
        tokio::spawn(async move {
            while let Ok((mut down, _)) = listener.accept().await {
                if !o.load(std::sync::atomic::Ordering::SeqCst) {
                    continue; // dropped: the client sees a refused connect
                }
                let Ok(mut up) = tokio::net::TcpStream::connect(&upstream).await else {
                    continue;
                };
                l.lock().expect("links").push(tokio::spawn(async move {
                    let _ = tokio::io::copy_bidirectional(&mut down, &mut up).await;
                }));
            }
        });
        Hop { port, open, links }
    }

    /// Drops every live connection; new ones are refused until `reopen`.
    fn cut(&self) {
        self.open.store(false, std::sync::atomic::Ordering::SeqCst);
        for link in self.links.lock().expect("links").drain(..) {
            link.abort();
        }
    }

    fn reopen(&self) {
        self.open.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// A lost connection is reconnected and counted once; a page published while
/// the server is unreachable reports its first unacknowledged event.
#[tokio::test(flavor = "multi_thread")]
async fn a_lost_connection_is_counted_and_unacked_publishes_are_reported() {
    let url = require_nats!();
    let hop = Hop::to(&url).await;
    let bus = NatsBus::connect(&format!("nats://127.0.0.1:{}", hop.port))
        .await
        .expect("connect through the hop");
    assert_eq!(bus.reconnects(), 0);
    hop.cut();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while bus.connected() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!bus.connected(), "the bus sees the cut");
    let e = bus
        .reply_ready(format!("antares.mcp.hop{}", nonce()), |p| Some(p.to_vec()))
        .await
        .expect_err("no server confirms the subscription");
    assert!(e.to_string().starts_with("bus error: "), "{e}");
    let tenant = TenantId::new(&format!("hop{}", nonce())).expect("tenant");
    let (sent, failed) = bus
        .publish_all(&[
            event(&tenant, "urn:hop:1", 1),
            event(&tenant, "urn:hop:2", 2),
        ])
        .await;
    assert_eq!(sent, 0, "no event was acknowledged");
    assert!(failed.is_some_and(|e| e.to_string().starts_with("bus error: ")));
    hop.reopen();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while bus.reconnects() == 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        bus.reconnects(),
        1,
        "one reconnect, the first connect not counted"
    );
    assert!(bus.connected());
}

/// An event the server cannot take (over its max_payload once its attribute list is counted)
/// stops the page there: the events before it are acknowledged, the error names the cause,
/// and nothing after it is sent.
#[tokio::test(flavor = "multi_thread")]
async fn publish_all_stops_at_an_event_too_large_to_send() {
    let url = require_nats!();
    let bus = NatsBus::connect(&url).await.expect("connect");
    let tenant = TenantId::new(&format!("big{}", nonce())).expect("tenant");
    let mut huge = event(&tenant, "urn:big:2", 2);
    // claim_check strips bodies, not the attribute list
    huge.changed_attrs = vec!["a".repeat(1024); 2048];
    let (sent, failed) = bus
        .publish_all(&[
            event(&tenant, "urn:big:1", 1),
            huge,
            event(&tenant, "urn:big:3", 3),
        ])
        .await;
    assert_eq!(sent, 1, "only the event before the oversized one");
    let e = failed.expect("the oversized event fails").to_string();
    assert!(e.starts_with("bus error: "), "{e}");
}

/// A page written whole before its acks are awaited (T-6608) reaches the
/// stream complete and in page order, so the matcher still sees an entity's
/// changes in the order the outbox holds them. The run prints the drain's
/// before/after cost: one awaited ack per event against one per page.
#[tokio::test(flavor = "multi_thread")]
async fn publish_all_keeps_page_order_and_acks_every_event() {
    let url = require_nats!();
    let bus = NatsBus::connect(&url).await.expect("connect");
    let n = nonce();
    let tenant = TenantId::new(&format!("pa{n}")).expect("tenant");
    let c = bus
        .consume_balanced(&format!("pa-{n}"))
        .await
        .expect("consumer");

    const PAGES: i64 = 10;
    const PAGE: i64 = 64;
    let page = |p: i64| -> Vec<ChangeEvent> {
        (0..PAGE)
            .map(|i| {
                let seq = p * PAGE + i + 1;
                event(&tenant, &format!("urn:pa:{n}:{}", seq % 3), seq)
            })
            .collect()
    };
    let started = std::time::Instant::now();
    for p in 0..PAGES {
        let events = page(p);
        let (published, failed) = bus.publish_all(&events).await;
        assert!(failed.is_none(), "page {p} failed: {failed:?}");
        assert_eq!(published, events.len(), "every event of page {p} acked");
    }
    let paged = started.elapsed();

    // the same number of events one awaited ack at a time, as the drain did
    let one_by_one = TenantId::new(&format!("pb{n}")).expect("tenant");
    let started = std::time::Instant::now();
    for seq in 1..=PAGES * PAGE {
        bus.publish(&event(&one_by_one, &format!("urn:pb:{n}:{seq}"), seq))
            .await
            .expect("publish");
    }
    let serial = started.elapsed();
    eprintln!(
        "{} events: publish_all in pages of {PAGE} {paged:?}, one awaited ack each {serial:?}",
        PAGES * PAGE
    );

    let mut seqs = Vec::new();
    let mut s = c.messages().await.expect("stream");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while seqs.len() < (PAGES * PAGE) as usize && tokio::time::Instant::now() < deadline {
        tokio::select! {
            Some(Ok(m)) = s.next() => {
                if let Some(got) = antares_bus::nats::decode(&m) {
                    if got.tenant.as_str() == tenant.as_str() {
                        seqs.push(got.seq);
                    }
                }
                m.ack().await.expect("ack");
            }
            _ = tokio::time::sleep(Duration::from_millis(200)) => {}
        }
    }
    let want: Vec<i64> = (1..=PAGES * PAGE).collect();
    assert_eq!(seqs, want, "every event once, in outbox order");
}

/// Messages the subscription bucket holds under `prefix` (a key prefix).
async fn subs_messages(url: &str, prefix: &str) -> u64 {
    let js = async_nats::jetstream::new(async_nats::connect(url).await.expect("connect"));
    let s = js
        .get_stream(format!("KV_{}", antares_bus::nats::SUBS_BUCKET))
        .await
        .expect("bucket stream");
    let mut subjects = s
        .info_with_subjects(format!("$KV.{}.{prefix}.>", antares_bus::nats::SUBS_BUCKET))
        .await
        .expect("info");
    let mut total = 0;
    while let Some(entry) = subjects.next().await {
        total += entry.expect("subject count").1 as u64;
    }
    total
}

/// T-7449: subscription churn (smoke creates and deletes thousands) must
/// leave O(live) entries. Before the fix every deleted id kept its
/// `present:false` put for good, 452,560 of them until jc-nats was OOM-killed.
#[tokio::test(flavor = "multi_thread")]
async fn subscription_churn_leaves_only_live_entries() {
    const N: u64 = 10_000;
    const LIVE: u64 = 10;
    let url = require_nats!();
    let bus = std::sync::Arc::new(NatsBus::connect(&url).await.expect("connect"));
    let kv = bus.subs_kv().await.expect("kv bucket");
    let prefix = format!("churn{}", nonce());
    let present = |p: bool| serde_json::to_vec(&serde_json::json!({"present": p})).expect("encode");
    futures_util::stream::iter(0..N)
        .map(|i| {
            let (kv, bus, key) = (kv.clone(), bus.clone(), format!("{prefix}.{i:016x}"));
            let (on, off) = (present(true), present(false));
            async move {
                kv.put(&key, on.into()).await.expect("create");
                if i >= LIVE {
                    let rev = kv.put(&key, off.into()).await.expect("delete");
                    bus.retire_subscription_key(&key, rev)
                        .await
                        .expect("retire");
                }
            }
        })
        .buffer_unordered(64)
        .collect::<Vec<()>>()
        .await;
    let left = subs_messages(&url, &prefix).await;
    assert_eq!(
        left,
        LIVE,
        "{N} created, {} deleted: only live entries stay",
        N - LIVE
    );
}

/// Retiring a delete keeps a later put of the same id: a subscription
/// created again inside the grace window must stay visible to a reopened watch.
#[tokio::test(flavor = "multi_thread")]
async fn retiring_a_delete_keeps_a_later_create() {
    let url = require_nats!();
    let bus = NatsBus::connect(&url).await.expect("connect");
    let kv = bus.subs_kv().await.expect("kv bucket");
    let prefix = format!("recreate{}", nonce());
    let key = format!("{prefix}.k");
    kv.put(&key, "1".into()).await.expect("create");
    let deleted = kv.put(&key, "0".into()).await.expect("delete");
    kv.put(&key, "2".into()).await.expect("create again");
    bus.retire_subscription_key(&key, deleted)
        .await
        .expect("retire");
    assert_eq!(subs_messages(&url, &prefix).await, 1);
    let entry = kv.get(&key).await.expect("get").expect("entry");
    assert_eq!(&entry[..], b"2");
}

/// The bucket carries the age backstop for entries a crash left unretired.
#[tokio::test(flavor = "multi_thread")]
async fn subscription_bucket_is_age_bounded() {
    let url = require_nats!();
    let bus = NatsBus::connect(&url).await.expect("connect");
    let kv = bus.subs_kv().await.expect("kv bucket");
    let status = kv.status().await.expect("status");
    assert_eq!(status.max_age(), antares_bus::nats::SUBS_MAX_AGE);
}
