//! amqp_0_9 input against a real RabbitMQ (the CI ekiben job's rabbitmq service). Runs where
//! AMQP_TEST_URL names a broker (amqp://user:password@127.0.0.1:5672/%2f), as the S3 tests run
//! where S3_TEST_ENDPOINT names a store.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use ekiben_core::{
    BatchError, ComponentConfig, CoreError, Input, OrgId, OrgQuotas, Registry, ResourceManager,
    StreamContext, StreamMetrics,
};
use ekiben_io_amqp::AmqpInput;
use lapin::options::{BasicPublishOptions, QueueDeclareOptions};
use lapin::types::FieldTable;
use lapin::{BasicProperties, Connection, ConnectionProperties};
use serde_json::json;

fn ctx() -> StreamContext {
    let resources = ResourceManager::new();
    // the broker listens on loopback, which only an organization with allow_private_ips reaches
    let org = resources.set_org_quotas(
        &OrgId::new("org-amqp"),
        OrgQuotas {
            allow_private_ips: true,
            ..Default::default()
        },
    );
    StreamContext {
        stream_id: "amqp-test".into(),
        org,
        resources,
        registry: Arc::new(Registry::new()),
        blobl_env: ekiben_bloblang::Environment::new(),
        metrics: StreamMetrics::new("amqp-test", "org-amqp"),
        label: String::new(),
        path: "root.input".into(),
        file_root: None,
        env_vars: Arc::new(HashMap::new()),
        shutdown_rx: tokio::sync::watch::channel(false).1,
    }
}

async fn next(
    input: &mut AmqpInput,
    c: &StreamContext,
) -> (ekiben_core::Batch, ekiben_core::AckResponder) {
    for _ in 0..20 {
        match input.read_batch(c).await {
            Ok(b) => return b,
            Err(CoreError::Timeout) => continue,
            Err(e) => panic!("read: {e}"),
        }
    }
    panic!("no message in 20 s");
}

fn text(b: &ekiben_core::Batch) -> String {
    String::from_utf8_lossy(&b[0].as_bytes_ref()).into_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn consumes_acks_requeues_and_rejects() {
    let Ok(url) = std::env::var("AMQP_TEST_URL") else {
        eprintln!("AMQP_TEST_URL unset: skipped");
        return;
    };
    let queue = format!("ekiben-{}", std::process::id());
    let conn = Connection::connect(&url, ConnectionProperties::default())
        .await
        .expect("publisher connects");
    let ch = conn.create_channel().await.expect("channel");
    ch.queue_declare(
        queue.clone().into(),
        QueueDeclareOptions::default(),
        FieldTable::default(),
    )
    .await
    .expect("declare");
    for body in ["one", "two", "three"] {
        ch.basic_publish(
            "".into(),
            queue.clone().into(),
            BasicPublishOptions::default(),
            body.as_bytes(),
            BasicProperties::default().with_content_type("text/plain".into()),
        )
        .await
        .expect("publish")
        .await
        .expect("confirm");
    }

    let mut input = AmqpInput::from_config(&ComponentConfig {
        label: String::new(),
        component_type: "amqp_0_9".into(),
        config: json!({
            "urls": [url],
            "queue": queue,
            "prefetch_count": 1,
            "nack_reject_patterns": ["^poison"],
        }),
        processors: Vec::new(),
    })
    .expect("builds");
    let c = ctx();

    let (b, ack) = next(&mut input, &c).await;
    assert_eq!(text(&b), "one");
    assert_eq!(b[0].meta_get_str("amqp_content_type"), "text/plain");
    assert_eq!(b[0].meta_get_str("amqp_routing_key"), queue.as_str());
    assert_eq!(b[0].meta_get_str("amqp_redelivered"), "false");
    ack.ack();

    // A failed message goes back to the queue and comes again, redelivered.
    let (b, ack) = next(&mut input, &c).await;
    assert_eq!(text(&b), "two");
    ack.nack(BatchError::new(b, "output down".to_string()));
    let (b, ack) = next(&mut input, &c).await;
    assert_eq!(text(&b), "two");
    assert_eq!(b[0].meta_get_str("amqp_redelivered"), "true");
    ack.ack();

    // An error matching nack_reject_patterns drops it.
    let (b, ack) = next(&mut input, &c).await;
    assert_eq!(text(&b), "three");
    ack.nack(BatchError::new(b, "poison message".to_string()));

    tokio::time::sleep(Duration::from_millis(500)).await;
    let q = ch
        .queue_declare(
            queue.clone().into(),
            QueueDeclareOptions {
                passive: true,
                ..QueueDeclareOptions::default()
            },
            FieldTable::default(),
        )
        .await
        .expect("passive declare");
    assert_eq!(q.message_count(), 0, "every message acked or rejected");
    input.close().await.expect("close");
    let _ = ch.queue_delete(queue.into(), Default::default()).await;
}
