//! The consumer against the scripted broker: every refusal a cluster or a group coordinator can
//! answer, offset resets, nacks racing a fetch, and the TLS + SCRAM path from the config.

use std::collections::BTreeSet;

use bytes::Bytes;
use ekiben_core::{BatchError, Registry};
use serde_json::{json, Value as JsonValue};

use super::*;
use crate::client::Mechanism;
use crate::tests::client::{localhost_tls, org, scram_broker};
use crate::tests::conf;
use crate::tests::fake::{self, Fake, Reply, Topic};
use crate::wire::Dec;

const G: &str = "org-kafka.g";

fn input(f: &Fake, extra: JsonValue) -> KafkaInput {
    let mut c = json!({"seed_brokers": [f.seed()], "topics": ["t"]});
    if let (Some(o), Some(x)) = (c.as_object_mut(), extra.as_object()) {
        o.extend(x.clone());
    }
    KafkaInput::new(&conf(c), &org()).expect("input")
}

fn group(f: &Fake) -> KafkaInput {
    input(f, json!({"consumer_group": "g"}))
}

async fn read(i: &mut KafkaInput) -> (Batch, AckResponder) {
    for _ in 0..20 {
        if let Some(b) = i.step().await.expect("step") {
            return b;
        }
    }
    panic!("no batch in 20 steps");
}

async fn refusal(i: &mut KafkaInput) -> CoreError {
    i.step().await.err().expect("refused")
}

fn values(b: &Batch) -> Vec<String> {
    b.iter()
        .map(|p| String::from_utf8_lossy(&p.as_bytes_ref()).into_owned())
        .collect()
}

fn topic(name: &str, parts: &[i32]) -> Topic {
    Topic {
        name: name.into(),
        code: 0,
        parts: parts.iter().map(|p| (0, *p, 1)).collect(),
    }
}

fn assigned(a: &[(&str, &[i32])]) -> Vec<u8> {
    group::assignment(
        &a.iter()
            .map(|(t, ps)| (t.to_string(), ps.to_vec()))
            .collect(),
    )
}

/// The assignments a SyncGroup request hands out, by member.
fn sync_assignments(body: &Bytes) -> Vec<(String, group::Assignment)> {
    let mut d = Dec(body.clone());
    d.str().expect("group");
    d.i32().expect("generation");
    d.str().expect("member");
    d.array(|d| Ok((d.str()?, d.bytes()?)))
        .expect("assignments")
        .into_iter()
        .map(|(m, a)| (m, group::decode_assignment(a).expect("assignment")))
        .collect()
}

#[tokio::test]
async fn a_topic_the_user_may_not_read_is_a_configuration_error() {
    let f = Fake::start().await;
    f.lock().topics[0].code = 29;
    let mut i = input(&f, json!({}));
    let e = i.read_batch(&org()).await.err().expect("refused");
    assert!(matches!(e, CoreError::Config(_)), "{e:?}");
    assert!(
        e.to_string()
            .contains("topic t: TOPIC_AUTHORIZATION_FAILED; the SASL user may not read it"),
        "{e}"
    );
}

#[tokio::test]
async fn an_offset_the_leader_cannot_list_marks_the_metadata_stale() {
    let f = Fake::start().await;
    f.push(
        crate::client::LIST_OFFSETS,
        fake::partition_code(crate::client::LIST_OFFSETS, "t", 0, 6),
    );
    let mut i = input(&f, json!({}));
    let e = refusal(&mut i).await.to_string();
    assert!(
        e.contains("listing offsets of t/0: NOT_LEADER_OR_FOLLOWER"),
        "{e}"
    );
    assert!(i.stale_meta);
}

#[tokio::test]
async fn patterns_match_the_cluster_s_topics_but_never_internal_ones() {
    let f = Fake::start().await;
    f.lock().topics = vec![
        topic("t", &[0]),
        topic("traffic-a", &[0]),
        topic("__consumer_offsets", &[0]),
    ];
    f.produce("traffic-a", 0, &["v"]);
    let mut i = input(
        &f,
        json!({"topics": ["traffic-.*|__.*"], "regexp_topics": true}),
    );
    let (b, _) = read(&mut i).await;
    assert_eq!(b[0].meta_get_str("kafka_topic"), "traffic-a");
    assert_eq!(i.subscribed(), vec!["traffic-a".to_string()]);

    let mut i = input(&f, json!({"topics": ["nothing-.*"], "regexp_topics": true}));
    let e = refusal(&mut i).await.to_string();
    assert!(e.contains("no topic matches topics yet"), "{e}");
}

#[tokio::test]
async fn finding_the_coordinator_fails_with_the_reason() {
    type Setup = Box<dyn Fn(&Fake)>;
    let cases: Vec<(Setup, &str, bool)> = vec![
        (
            Box::new(|f| f.lock().brokers.clear()),
            "the cluster advertises no broker",
            false,
        ),
        (
            Box::new(|f| {
                f.push(
                    crate::client::FIND_COORDINATOR,
                    fake::coordinator(30, 1, "127.0.0.1", 1),
                )
            }),
            "GROUP_AUTHORIZATION_FAILED for group org-kafka.g; grant the SASL user the group org-kafka.g",
            true,
        ),
        (
            Box::new(|f| {
                f.push(
                    crate::client::FIND_COORDINATOR,
                    fake::coordinator(15, 1, "127.0.0.1", 1),
                )
            }),
            "finding the coordinator of org-kafka.g: COORDINATOR_NOT_AVAILABLE",
            false,
        ),
        (
            Box::new(|f| {
                f.push(
                    crate::client::FIND_COORDINATOR,
                    fake::coordinator(0, 1, "127.0.0.1", 70_000),
                )
            }),
            "the coordinator's port is out of range",
            false,
        ),
    ];
    for (setup, want, config) in cases {
        let f = Fake::start().await;
        setup(&f);
        let mut i = group(&f);
        let e = refusal(&mut i).await;
        assert_eq!(matches!(e, CoreError::Config(_)), config, "{want}: {e:?}");
        assert!(e.to_string().contains(want), "{want}: {e}");
    }
}

#[tokio::test]
async fn join_retries_a_rebalance_and_an_unknown_member() {
    let f = Fake::start().await;
    f.push(crate::client::JOIN_GROUP, fake::joined(25, "", "", "", &[]));
    f.push(crate::client::JOIN_GROUP, fake::joined(27, "", "", "", &[]));
    f.produce("t", 0, &["v"]);
    let mut i = group(&f);
    let (b, _) = read(&mut i).await;
    assert_eq!(values(&b), vec!["v"]);
    assert_eq!(f.seen(crate::client::JOIN_GROUP).len(), 3);
    assert_eq!(i.member_id, "m-1");
}

#[tokio::test]
async fn join_refusals_name_what_to_change() {
    let sub = group::subscription(&["t".to_string()]);
    let cases = vec![
        (
            fake::joined(30, "", "", "", &[]),
            "GROUP_AUTHORIZATION_FAILED for group org-kafka.g; grant the SASL user that group",
            true,
        ),
        (
            fake::joined(23, "", "", "", &[]),
            "INCONSISTENT_GROUP_PROTOCOL); give this input its own consumer_group",
            true,
        ),
        (
            fake::joined(24, "", "", "", &[]),
            "joining org-kafka.g: INVALID_GROUP_ID",
            false,
        ),
        (
            fake::joined(0, "roundrobin", "m-1", "m-1", &[("m-1", sub)]),
            "group org-kafka.g chose the assignor roundrobin; this input offers only range",
            false,
        ),
    ];
    for (reply, want, config) in cases {
        let f = Fake::start().await;
        f.push(crate::client::JOIN_GROUP, reply);
        let mut i = group(&f);
        let e = refusal(&mut i).await;
        assert_eq!(matches!(e, CoreError::Config(_)), config, "{want}: {e:?}");
        assert!(e.to_string().contains(want), "{want}: {e}");
    }
}

#[tokio::test]
async fn a_follower_reads_the_partitions_the_leader_assigned() {
    let f = Fake::start().await;
    f.push(
        crate::client::JOIN_GROUP,
        fake::joined(0, "range", "m-0", "m-1", &[]),
    );
    f.push(
        crate::client::SYNC_GROUP,
        fake::synced(0, &assigned(&[("t", &[0])])),
    );
    f.produce("t", 0, &["v"]);
    let mut i = group(&f);
    let (b, _) = read(&mut i).await;
    assert_eq!(values(&b), vec!["v"]);
    let sync = f.seen(crate::client::SYNC_GROUP);
    assert!(
        sync_assignments(&sync[0]).is_empty(),
        "a follower assigns nothing"
    );
}

#[tokio::test]
async fn a_member_assigned_nothing_waits_connected() {
    let f = Fake::start().await;
    f.push(
        crate::client::JOIN_GROUP,
        fake::joined(0, "range", "m-0", "m-1", &[]),
    );
    f.push(crate::client::SYNC_GROUP, fake::synced(0, &[]));
    f.produce("t", 0, &["v"]);
    let mut i = group(&f);
    assert!(i.step().await.expect("step").is_none());
    assert!(i.is_connected());
    assert!(i.leaders.is_empty());
    assert!(f.seen(crate::client::FETCH).is_empty());
}

#[tokio::test]
async fn the_leader_assigns_the_topics_other_members_subscribe_to() {
    let f = Fake::start().await;
    f.lock().topics = vec![topic("t", &[0]), topic("u", &[0, 1])];
    let members = [
        ("m-1", group::subscription(&["t".to_string()])),
        ("m-2", group::subscription(&["u".to_string()])),
    ];
    f.push(
        crate::client::JOIN_GROUP,
        fake::joined(0, "range", "m-1", "m-1", &members),
    );
    f.produce("t", 0, &["v"]);
    let mut i = group(&f);
    read(&mut i).await;
    let sync = sync_assignments(&f.seen(crate::client::SYNC_GROUP)[0]);
    let want: group::Assignment = [("u".to_string(), vec![0, 1])].into_iter().collect();
    assert!(sync.contains(&("m-2".to_string(), want)), "{sync:?}");
    // u is not among this member's topics: the leader read its metadata to assign it
    let asked: Vec<Vec<String>> = f
        .seen(crate::client::METADATA)
        .iter()
        .map(|b| Dec(b.clone()).array(|d| d.str()).expect("topics"))
        .collect();
    assert!(
        asked.contains(&vec!["t".to_string(), "u".to_string()]),
        "{asked:?}"
    );

    // a coordinator the cluster does not advertise is lost when the leader rereads the metadata
    let f = Fake::start().await;
    f.lock().topics = vec![topic("t", &[0]), topic("u", &[0, 1])];
    let port = i32::from(f.port);
    f.push(
        crate::client::FIND_COORDINATOR,
        fake::coordinator(0, 7, "127.0.0.1", port),
    );
    f.push(
        crate::client::JOIN_GROUP,
        fake::joined(0, "range", "m-1", "m-1", &members),
    );
    let mut i = group(&f);
    let e = refusal(&mut i).await.to_string();
    assert!(e.contains("broker 7 is not in the metadata"), "{e}");
}

#[tokio::test]
async fn sync_and_offset_fetch_refusals() {
    let f = Fake::start().await;
    f.push(crate::client::SYNC_GROUP, fake::synced(25, &[]));
    let mut i = group(&f);
    let e = refusal(&mut i).await.to_string();
    assert!(
        e.contains(&format!("syncing {G}: UNKNOWN_MEMBER_ID")),
        "{e}"
    );
    assert_eq!(
        i.member_id, "",
        "an unknown member joins again as a new one"
    );

    let f = Fake::start().await;
    f.push(crate::client::SYNC_GROUP, fake::synced(27, &[]));
    let mut i = group(&f);
    let e = refusal(&mut i).await.to_string();
    assert!(
        e.contains(&format!("syncing {G}: REBALANCE_IN_PROGRESS")),
        "{e}"
    );
    assert_eq!(i.member_id, "m-1", "a rebalance keeps the member id");

    let f = Fake::start().await;
    f.push(crate::client::OFFSET_FETCH, fake::offset("t", 0, -1, 16));
    let mut i = group(&f);
    let e = refusal(&mut i).await.to_string();
    assert!(
        e.contains("reading the offset of t/0: NOT_COORDINATOR"),
        "{e}"
    );

    let f = Fake::start().await;
    f.push(
        crate::client::SYNC_GROUP,
        fake::synced(0, &assigned(&[("t", &[5])])),
    );
    f.push(crate::client::OFFSET_FETCH, fake::offset("t", 5, -1, 0));
    let mut i = group(&f);
    let e = refusal(&mut i).await.to_string();
    assert!(e.contains("t/5 has no leader in the metadata"), "{e}");
}

#[tokio::test]
async fn reading_resumes_at_the_committed_offset_and_commits_below_unacked_batches() {
    let f = Fake::start().await;
    f.produce("t", 0, &["a", "b", "c", "d", "e"]);
    f.push(crate::client::OFFSET_FETCH, fake::offset("t", 0, 3, 0));
    let mut i = group(&f);
    let (b, ack) = read(&mut i).await;
    assert_eq!(values(&b), vec!["d", "e"]);
    assert_eq!(b[0].meta_get_str("kafka_offset"), "3");
    assert_eq!(b[0].meta_get_str("kafka_lag"), "1");
    i.commit().await.expect("nothing acked: nothing to commit");
    assert!(f.seen(crate::client::OFFSET_COMMIT).is_empty());
    ack.ack();
    i.commit().await.expect("commit");
    let commits = f.seen(crate::client::OFFSET_COMMIT);
    assert_eq!(commits.len(), 1);
    assert_eq!(
        &commits[0][commits[0].len() - 10..commits[0].len() - 2],
        &5i64.to_be_bytes()
    );
    assert_eq!(i.lock().parts[&("t".to_string(), 0)].committed, 5);
}

#[tokio::test]
async fn a_stale_generation_on_commit_rejoins_and_other_codes_are_reported() {
    let f = Fake::start().await;
    f.produce("t", 0, &["a"]);
    let mut i = group(&f);
    let (_, ack) = read(&mut i).await;
    ack.ack();
    f.push(
        crate::client::OFFSET_COMMIT,
        fake::partition_code(crate::client::OFFSET_COMMIT, "t", 0, 22),
    );
    let e = i.commit().await.expect_err("refused").to_string();
    assert!(
        e.contains("committing offsets: ILLEGAL_GENERATION; rejoining the group"),
        "{e}"
    );
    assert!(i.needs_rejoin());
    f.produce("t", 0, &["b"]);
    let (b, _) = read(&mut i).await;
    assert_eq!(f.seen(crate::client::JOIN_GROUP).len(), 2, "rejoined");
    assert_eq!(
        values(&b),
        vec!["b"],
        "leaving the assignment commits the ack first"
    );

    let f = Fake::start().await;
    f.produce("t", 0, &["a"]);
    let mut i = group(&f);
    let (_, ack) = read(&mut i).await;
    ack.ack();
    f.push(
        crate::client::OFFSET_COMMIT,
        fake::partition_code(crate::client::OFFSET_COMMIT, "t", 0, 28),
    );
    let e = i.commit().await.expect_err("refused").to_string();
    assert!(e.contains("committing offsets: an error"), "{e}");
    assert!(!i.needs_rejoin());
}

#[tokio::test]
async fn fetch_answers_mark_the_metadata_stale_skip_or_refuse() {
    let f = Fake::start().await;
    let mut i = input(&f, json!({}));
    assert!(i.step().await.expect("assigned").is_none());
    let fetch = crate::client::FETCH;

    f.push(fetch, Reply::Close);
    refusal(&mut i).await;
    assert!(i.stale_meta, "a failed fetch rereads the metadata");

    for (c, stale) in [(3, true), (6, true), (99, false)] {
        assert!(i.step().await.expect("refreshed").is_none());
        assert!(!i.stale_meta);
        f.push(fetch, fake::fetched("t", 0, c, 0, &[]));
        assert!(i.step().await.expect("skipped").is_none());
        assert_eq!(i.stale_meta, stale, "code {c}");
    }

    // a partition the fetch did not ask for is ignored
    f.push(fetch, fake::fetched("t", 9, 0, 1, &[]));
    assert!(i.step().await.expect("ignored").is_none());

    f.push(fetch, Reply::Body(vec![0, 0]));
    let e = refusal(&mut i).await.to_string();
    assert!(e.contains("malformed Kafka response: truncated"), "{e}");

    f.push(fetch, fake::fetched("t", 0, 29, 0, &[]));
    let e = refusal(&mut i).await;
    assert!(matches!(e, CoreError::Config(_)), "{e:?}");
    assert!(
        e.to_string()
            .contains("topic t: TOPIC_AUTHORIZATION_FAILED"),
        "{e}"
    );
}

#[tokio::test]
async fn an_offset_out_of_range_resets_to_the_oldest() {
    let f = Fake::start().await;
    f.produce("t", 0, &["a", "b", "c"]);
    f.push(crate::client::OFFSET_FETCH, fake::offset("t", 0, 10, 0));
    f.push(crate::client::FETCH, fake::fetched("t", 0, 1, 3, &[]));
    let mut i = group(&f);
    let (b, _) = read(&mut i).await;
    assert_eq!(values(&b), vec!["a", "b", "c"]);
}

#[tokio::test]
async fn a_nack_during_a_fetch_drops_what_the_fetch_brought() {
    let f = Fake::start().await;
    f.produce("t", 0, &["a", "b"]);
    let mut i = input(&f, json!({}));
    let (b, ack) = read(&mut i).await;
    assert_eq!(values(&b), vec!["a", "b"]);
    f.produce("t", 0, &["c"]);
    let held = Arc::new(Mutex::new(Some((b, ack))));
    let nack = held.clone();
    f.lock().hook = Some(Box::new(move |api, _| {
        if api == crate::client::FETCH {
            if let Some((b, ack)) = nack.lock().expect("held").take() {
                ack.nack(BatchError::new(b, "the output refused"));
            }
        }
        None
    }));
    assert!(
        i.step().await.expect("step").is_none(),
        "c was fetched for a rewound partition"
    );
    let (b, _) = read(&mut i).await;
    assert_eq!(values(&b), vec!["a", "b", "c"]);
}

#[tokio::test]
async fn a_batch_queued_before_a_nack_is_not_delivered() {
    let f = Fake::start().await;
    f.lock().topics = vec![topic("t", &[0, 1])];
    f.produce("t", 0, &["p0-a"]);
    f.produce("t", 1, &["p1-a"]);
    let mut i = input(&f, json!({}));
    let mut first = std::collections::HashMap::new();
    for _ in 0..2 {
        let (b, ack) = read(&mut i).await;
        first.insert(b[0].meta_get_str("kafka_partition"), (b, ack));
    }
    f.produce("t", 0, &["p0-b"]);
    f.produce("t", 1, &["p1-b"]);
    // one fetch brings both partitions: one is delivered, the other waits in the queue
    let (b, _) = read(&mut i).await;
    let delivered = b[0].meta_get_str("kafka_partition");
    let queued = if delivered == "0" { "1" } else { "0" };
    assert_eq!(i.ready.len(), 1);
    let (b, ack) = first.remove(queued).expect("first batch");
    ack.nack(BatchError::new(b, "the output refused"));
    let (b, _) = read(&mut i).await;
    assert_eq!(
        values(&b),
        vec![format!("p{queued}-a"), format!("p{queued}-b")],
        "the queued batch was stale; the partition is read again from the nack"
    );
}

#[tokio::test]
async fn multi_header_keeps_every_value_of_a_header() {
    let f = Fake::start().await;
    f.produce("t", 0, &["v"]);
    let mut i = input(&f, json!({"multi_header": true}));
    let (b, _) = read(&mut i).await;
    assert_eq!(
        b[0].meta_get("h"),
        Some(&Value::Array(vec![Value::String("x".into())].into()))
    );
}

/// A record sent without a key carries no kafka_key, the record beside it keeps its own.
#[tokio::test]
async fn a_record_without_a_key_has_no_kafka_key() {
    let f = Fake::start().await;
    f.produce("t", 0, &["nokey:bare", "keyed"]);
    let mut i = input(&f, json!({}));
    let (b, _) = read(&mut i).await;
    assert_eq!(values(&b), vec!["bare", "keyed"]);
    assert_eq!(b[0].meta_get("kafka_key"), None);
    assert_eq!(b[1].meta_get_str("kafka_key"), "k");
}

/// Heartbeats the coordinator answers keep the member in the group: no rejoin.
#[tokio::test(start_paused = true)]
async fn answered_heartbeats_keep_the_member() {
    let f = Fake::start().await;
    f.produce("t", 0, &["a"]);
    let mut i = group(&f);
    let (_, ack) = read(&mut i).await;
    ack.ack();
    tokio::time::sleep(HEARTBEAT_EVERY * 2 + Duration::from_secs(1)).await;
    assert!(f.seen(crate::client::HEARTBEAT).len() >= 2);
    assert!(!i.needs_rejoin());
}

#[tokio::test]
async fn without_a_group_a_new_partition_is_read_too() {
    let f = Fake::start().await;
    f.produce("t", 0, &["a"]);
    let mut i = input(&f, json!({"metadata_max_age": "0s"}));
    let (b, ack) = read(&mut i).await;
    assert_eq!(values(&b), vec!["a"]);
    ack.ack();
    f.lock().topics[0].parts.push((0, 1, 1));
    f.produce("t", 1, &["b"]);
    let mut partitions = BTreeSet::new();
    for _ in 0..2 {
        let (b, _) = read(&mut i).await;
        partitions.insert(b[0].meta_get_str("kafka_partition"));
    }
    assert!(partitions.contains("1"), "{partitions:?}");
    assert_eq!(i.lock().parts.len(), 2);
}

#[tokio::test]
async fn an_ack_after_a_panicked_ack_callback_still_counts() {
    let f = Fake::start().await;
    f.produce("t", 0, &["a"]);
    let mut i = input(&f, json!({}));
    let (_, ack) = read(&mut i).await;
    let shared = i.shared.clone();
    let _ = std::thread::spawn(move || {
        let _held = shared.lock();
        panic!("an ack callback panicked");
    })
    .join();
    assert!(i.shared.is_poisoned());
    ack.ack();
    assert_eq!(i.lock().parts[&("t".to_string(), 0)].unacked(), 0);
}

#[tokio::test]
async fn the_input_reports_its_type_label_and_connection() {
    let f = Fake::start().await;
    f.produce("t", 0, &["a"]);
    let mut c = conf(json!({"seed_brokers": [f.seed()], "topics": ["t"]}));
    c.label = "orders".into();
    let mut i = KafkaInput::new(&c, &org()).expect("input");
    assert_eq!(
        (i.input_type(), i.label(), i.is_connected()),
        ("kafka_franz", "orders", false)
    );
    read(&mut i).await;
    assert!(i.is_connected());

    let r = Registry::new();
    crate::register_all(&r);
    let built = r
        .build_input(
            &conf(json!({"seed_brokers": [f.seed()], "topics": ["t"]})),
            &org(),
        )
        .expect("built");
    assert_eq!(built.input_type(), "kafka_franz");
    let e = r
        .build_input(
            &conf(json!({"seed_brokers": [f.seed()], "topics": []})),
            &org(),
        )
        .err()
        .expect("refused");
    assert!(matches!(e, CoreError::Config(_)), "{e:?}");
}

/// A heartbeat the coordinator drops makes the member rejoin on its next read.
#[tokio::test(start_paused = true)]
async fn a_lost_heartbeat_rejoins_the_group() {
    let f = Fake::start().await;
    f.produce("t", 0, &["a"]);
    let mut i = group(&f);
    let (_, ack) = read(&mut i).await;
    ack.ack();
    f.push(crate::client::HEARTBEAT, Reply::Close);
    tokio::time::sleep(HEARTBEAT_EVERY + Duration::from_secs(1)).await;
    assert!(i.needs_rejoin());
    f.produce("t", 0, &["b"]);
    let (b, _) = read(&mut i).await;
    assert_eq!(values(&b), vec!["b"]);
    assert_eq!(f.seen(crate::client::JOIN_GROUP).len(), 2);
}

#[tokio::test]
async fn a_tls_broker_with_scram_sha_512_is_read_from_the_config() {
    let (pem, _, acceptor) = localhost_tls();
    let f = Fake::start_with(Some(acceptor)).await;
    f.lock().hook = Some(scram_broker(Mechanism::ScramSha512, "pencil"));
    // the certificate names localhost, so the cluster advertises that name
    f.lock().brokers = vec![(1, "localhost".into(), i32::from(f.port))];
    f.produce("t", 0, &["over tls"]);
    let mut i = KafkaInput::new(
        &conf(json!({
            "seed_brokers": [format!("localhost:{}", f.port)],
            "topics": ["t"],
            "consumer_group": "g",
            "tls": {"enabled": true, "root_cas": pem},
            "sasl": [{"mechanism": "SCRAM-SHA-512", "username": "ingest", "password": "pencil"}],
        })),
        &org(),
    )
    .expect("input");
    let (b, _) = read(&mut i).await;
    assert_eq!(values(&b), vec!["over tls"]);
}

#[tokio::test]
async fn a_topic_the_cluster_does_not_know_is_skipped() {
    let f = Fake::start().await;
    f.lock().topics.push(Topic {
        name: "gone".into(),
        code: 3,
        parts: Vec::new(),
    });
    f.produce("t", 0, &["v"]);
    let mut i = input(&f, json!({"topics": ["t", "gone"]}));
    let (b, _) = read(&mut i).await;
    assert_eq!(values(&b), vec!["v"]);
    assert_eq!(
        i.cluster.meta.errors,
        vec![("gone".to_string(), "UNKNOWN_TOPIC_OR_PARTITION")]
    );
}

#[tokio::test]
async fn an_assignment_that_failed_half_way_is_retried_from_the_join() {
    let f = Fake::start().await;
    f.produce("t", 0, &["v"]);
    f.push(
        crate::client::LIST_OFFSETS,
        fake::partition_code(crate::client::LIST_OFFSETS, "t", 0, 6),
    );
    let mut i = group(&f);
    refusal(&mut i).await;
    assert!(i.member.is_some(), "the join itself succeeded");
    let (b, _) = read(&mut i).await;
    assert_eq!(values(&b), vec!["v"]);
    assert_eq!(f.seen(crate::client::JOIN_GROUP).len(), 2);
}

#[tokio::test]
async fn closing_leaves_the_group_even_when_the_last_commit_fails() {
    let f = Fake::start().await;
    f.produce("t", 0, &["v"]);
    let mut i = group(&f);
    let (_, ack) = read(&mut i).await;
    ack.ack();
    f.push(crate::client::OFFSET_COMMIT, Reply::Close);
    i.close().await.expect("closed");
    assert_eq!(f.seen(crate::client::OFFSET_COMMIT).len(), 1);
    assert_eq!(f.seen(crate::client::LEAVE_GROUP).len(), 1);
    assert!(i.member.is_none());
}

#[tokio::test]
async fn a_new_partition_rejoins_the_group_even_when_the_commit_fails() {
    let f = Fake::start().await;
    f.produce("t", 0, &["a"]);
    let mut i = group(&f);
    let (_, ack) = read(&mut i).await;
    ack.ack();
    f.lock().topics[0].parts.push((0, 1, 1));
    f.push(crate::client::OFFSET_COMMIT, Reply::Close);
    i.stale_meta = true;
    let (b, _) = read(&mut i).await;
    assert_eq!(f.seen(crate::client::JOIN_GROUP).len(), 2);
    assert_eq!(values(&b), vec!["a"], "nothing was committed: read again");
    assert_eq!(i.lock().parts.len(), 2);
}

#[tokio::test]
async fn a_failed_periodic_commit_does_not_stop_reading() {
    let f = Fake::start().await;
    f.produce("t", 0, &["a"]);
    let mut i = input(&f, json!({"consumer_group": "g", "commit_period": "0s"}));
    let (_, ack) = read(&mut i).await;
    ack.ack();
    f.push(
        crate::client::OFFSET_COMMIT,
        fake::partition_code(crate::client::OFFSET_COMMIT, "t", 0, 28),
    );
    f.produce("t", 0, &["b"]);
    let (b, _) = read(&mut i).await;
    assert_eq!(values(&b), vec!["b"]);
    assert_eq!(f.seen(crate::client::OFFSET_COMMIT).len(), 1);
}

#[tokio::test]
async fn a_partition_at_its_checkpoint_limit_is_not_fetched() {
    let f = Fake::start().await;
    f.produce("t", 0, &["a"]);
    let mut i = input(&f, json!({"checkpoint_limit": 1}));
    let (_, ack) = read(&mut i).await;
    f.produce("t", 0, &["b"]);
    let fetches = f.seen(crate::client::FETCH).len();
    assert!(i.step().await.expect("step").is_none());
    assert_eq!(
        f.seen(crate::client::FETCH).len(),
        fetches,
        "one message is out"
    );
    ack.ack();
    let (b, _) = read(&mut i).await;
    assert_eq!(values(&b), vec!["b"]);
}

#[tokio::test]
async fn a_control_batch_moves_the_position_on() {
    let f = Fake::start().await;
    f.produce("t", 0, &["marker", "a"]);
    let marker = crate::records::encode::batch(0, 0, Some((7, true)), &[("", "")]);
    f.push(crate::client::FETCH, fake::fetched("t", 0, 0, 2, &marker));
    let mut i = input(&f, json!({}));
    assert!(i.step().await.expect("step").is_none());
    let (b, _) = read(&mut i).await;
    assert_eq!(values(&b), vec!["a"]);
}

#[tokio::test]
async fn read_committed_drops_an_aborted_transaction() {
    let f = Fake::start().await;
    let mut data = crate::records::encode::batch(0, 0, Some((7, false)), &[("k", "aborted")]);
    data.extend(crate::records::encode::batch(1, 0, None, &[("k", "plain")]));
    f.push(
        crate::client::FETCH,
        fake::fetched_aborted("t", 0, 0, 2, &[(7, 0)], &data),
    );
    let mut i = input(&f, json!({"transaction_isolation_level": "read_committed"}));
    let (b, _) = read(&mut i).await;
    assert_eq!(values(&b), vec!["plain"]);
}
