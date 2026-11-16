//! kafka_franz against a real broker (the CI ekiben job's redpanda). Runs where
//! KAFKA_TEST_BROKERS names one (127.0.0.1:19092), as the AMQP tests run where AMQP_TEST_URL
//! names a broker. The test creates its own topic and produces with the batch writer the unit
//! tests use.

use std::time::Duration;

use ekiben_core::{AckResponder, Batch, BatchError, CoreError, Input, OrgQuotas, StreamContext};
use serde_json::json;

use crate::client::{Cluster, ConnSettings};
use crate::records::encode::batch;
use crate::tests::{conf, ctx};
use crate::wire::Enc;
use crate::KafkaInput;

const CREATE_TOPICS: i16 = 19;
const PRODUCE: i16 = 0;
const T: Duration = Duration::from_secs(10);

fn org() -> StreamContext {
    // the broker listens on loopback, which only an organization with allow_private_ips reaches
    ctx(OrgQuotas {
        allow_private_ips: true,
        ..Default::default()
    })
}

async fn cluster(broker: &str, c: &StreamContext) -> Cluster {
    let (host, port) = broker.rsplit_once(':').expect("host:port");
    let s = ConnSettings {
        client_id: "ekiben-test".into(),
        tls: None,
        sasl: None,
        max_response: 1 << 20,
        request_timeout: T,
        org: c.org.clone(),
    };
    let mut cl = Cluster::new(s, vec![(host.into(), port.parse().expect("port"))]);
    cl.refresh(None).await.expect("metadata");
    cl
}

async fn create_topic(cl: &mut Cluster, topic: &str) {
    let mut e = Enc::default();
    e.array(&[()], |e, _| {
        e.str(topic).i32(2).i16(1).i32(0).i32(0);
    })
    .i32(10_000)
    .bool(false);
    let node = *cl.meta.brokers.keys().next().expect("a broker");
    let mut d = cl
        .call(node, CREATE_TOPICS, 2, &e.0, T)
        .await
        .expect("create");
    d.i32().expect("throttle");
    let codes = d
        .array(|d| {
            d.str()?;
            let c = d.i16()?;
            d.null_str()?;
            Ok(c)
        })
        .expect("answer");
    assert_eq!(codes, vec![0], "topic created");
}

async fn produce(cl: &mut Cluster, topic: &str, partition: i32, codec: i16, values: &[&str]) {
    cl.refresh(Some(&[topic.to_string()]))
        .await
        .expect("metadata");
    let leader = cl.meta.topics[topic]
        .iter()
        .find(|(p, _)| *p == partition)
        .expect("partition")
        .1;
    let recs: Vec<(&str, &str)> = values.iter().map(|v| ("key", *v)).collect();
    let records = batch(0, codec, None, &recs);
    let mut e = Enc::default();
    e.null_str().i16(-1).i32(10_000).array(&[()], |e, _| {
        e.str(topic).array(&[()], |e, _| {
            e.i32(partition).bytes(&records);
        });
    });
    let mut d = cl.call(leader, PRODUCE, 3, &e.0, T).await.expect("produce");
    let codes = d
        .array(|d| {
            d.str()?;
            d.array(|d| {
                d.i32()?;
                let c = d.i16()?;
                d.i64()?;
                d.i64()?;
                Ok(c)
            })
        })
        .expect("answer");
    assert_eq!(codes, vec![vec![0]], "produced");
}

async fn next(input: &mut KafkaInput, c: &StreamContext) -> (Batch, AckResponder) {
    for _ in 0..60 {
        match input.read_batch(c).await {
            Ok(b) => return b,
            Err(CoreError::Timeout) => continue,
            Err(e) => panic!("read: {e}"),
        }
    }
    panic!("no message in 60 reads");
}

fn texts(b: &Batch) -> Vec<String> {
    b.iter()
        .map(|p| String::from_utf8_lossy(&p.as_bytes_ref()).into_owned())
        .collect()
}

/// Reads until `n` messages arrived, acking each batch; the first batch is nacked once.
async fn read(
    input: &mut KafkaInput,
    c: &StreamContext,
    n: usize,
    nack_first: bool,
) -> Vec<String> {
    let mut got = Vec::new();
    let mut nacked = !nack_first;
    while got.len() < n {
        let (b, ack) = next(input, c).await;
        if !nacked {
            nacked = true;
            ack.nack(BatchError::new(b.clone(), "the output refused"));
            continue;
        }
        assert_eq!(b[0].meta_get_str("kafka_key"), "key");
        assert_eq!(b[0].meta_get_str("h"), "x");
        got.extend(texts(&b));
        ack.ack();
    }
    got.sort();
    got
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_group_consumes_replays_a_nack_and_resumes_from_its_commit() {
    let Ok(broker) = std::env::var("KAFKA_TEST_BROKERS") else {
        eprintln!("KAFKA_TEST_BROKERS unset: skipped");
        return;
    };
    let c = org();
    let topic = format!(
        "ekiben-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    );
    let mut cl = cluster(&broker, &c).await;
    create_topic(&mut cl, &topic).await;
    produce(&mut cl, &topic, 0, 0, &["a", "b", "c"]).await;
    produce(&mut cl, &topic, 1, 1, &["d", "e"]).await;

    let group = format!("g-{topic}");
    let cfg = json!({"seed_brokers": [broker], "topics": [topic], "consumer_group": group, "commit_period": "100ms"});
    let mut first = KafkaInput::new(&conf(cfg.clone()), &c).expect("builds");
    assert_eq!(
        read(&mut first, &c, 5, true).await,
        ["a", "b", "c", "d", "e"]
    );
    first.close().await.expect("close commits and leaves");

    produce(&mut cl, &topic, 0, 4, &["f"]).await;
    produce(&mut cl, &topic, 1, 3, &["g"]).await;
    let mut second = KafkaInput::new(&conf(cfg), &c).expect("builds");
    assert_eq!(
        read(&mut second, &c, 2, false).await,
        ["f", "g"],
        "resumes after the commit"
    );
    second.close().await.expect("close");

    // the committed group sits under the organization's prefix
    let mut e = Enc::default();
    e.str(&format!("org-kafka.{group}")).array(&[()], |e, _| {
        e.str(&topic).array(&[0i32], |e, p| {
            e.i32(*p);
        });
    });
    let gid = format!("org-kafka.{group}");
    let any = *cl.meta.brokers.keys().next().expect("a broker");
    let d = cl
        .call(
            any,
            crate::client::FIND_COORDINATOR,
            1,
            &crate::group::find_coordinator(&gid),
            T,
        )
        .await
        .expect("find coordinator");
    let (code, coordinator, _, _) = crate::group::decode_coordinator(d).expect("coordinator");
    assert_eq!(code, 0);
    let d = cl
        .call(coordinator, crate::client::OFFSET_FETCH, 1, &e.0, T)
        .await
        .expect("offset fetch");
    let offsets = crate::group::decode_offset_fetch(d).expect("offsets");
    assert_eq!(offsets[0].2, 4, "partition 0 committed past a, b, c, f");

    // without a group every partition is read from the oldest offset and nothing is committed
    let mut all = KafkaInput::new(
        &conf(json!({"seed_brokers": [broker], "topics": [topic]})),
        &c,
    )
    .expect("builds");
    assert_eq!(
        read(&mut all, &c, 7, false).await,
        ["a", "b", "c", "d", "e", "f", "g"]
    );
    all.close().await.expect("close");
}

/// A second member joining splits the partitions: the first sees the rebalance on its heartbeat,
/// rejoins, and each member reads one partition.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_member_takes_half_the_partitions() {
    let Ok(broker) = std::env::var("KAFKA_TEST_BROKERS") else {
        eprintln!("KAFKA_TEST_BROKERS unset: skipped");
        return;
    };
    let c = org();
    let topic = format!(
        "ekiben-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    );
    let mut cl = cluster(&broker, &c).await;
    create_topic(&mut cl, &topic).await;
    let cfg = json!({"seed_brokers": [broker], "topics": [topic], "consumer_group": "split", "commit_period": "100ms"});
    let mut a = KafkaInput::new(&conf(cfg.clone()), &c).expect("builds");
    produce(&mut cl, &topic, 0, 0, &["warm"]).await;
    assert_eq!(read(&mut a, &c, 1, false).await, ["warm"]);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut members = Vec::new();
    for (name, mut input) in [
        ("a", a),
        ("b", KafkaInput::new(&conf(cfg), &c).expect("builds")),
    ] {
        let (tx, c) = (tx.clone(), org());
        members.push(tokio::spawn(async move {
            loop {
                match input.read_batch(&c).await {
                    Ok((b, ack)) => {
                        for p in &b {
                            let _ = tx.send((
                                name,
                                p.meta_get_str("kafka_partition"),
                                texts(&vec![p.clone()]).remove(0),
                            ));
                        }
                        ack.ack();
                    }
                    Err(CoreError::Timeout) => {}
                    Err(e) => panic!("{name}: {e}"),
                }
            }
        }));
    }
    // produce until both members have read one partition each
    let mut seen = std::collections::BTreeMap::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    let mut round = 0;
    while seen.len() < 2 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "no split in 90 s: {seen:?}"
        );
        round += 1;
        produce(&mut cl, &topic, 0, 0, &[&format!("p0-{round}")]).await;
        produce(&mut cl, &topic, 1, 0, &[&format!("p1-{round}")]).await;
        let until = tokio::time::Instant::now() + Duration::from_secs(3);
        while let Ok(Some((name, partition, _))) = tokio::time::timeout_at(until, rx.recv()).await {
            if round > 1 {
                seen.insert(partition, name);
            }
        }
        if seen
            .values()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            < 2
        {
            seen.clear();
        }
    }
    assert_ne!(
        seen["0"], seen["1"],
        "each member owns one partition: {seen:?}"
    );
    for m in members {
        m.abort();
    }
}
