//! The MQTT capture store against a real PostgreSQL (ADR-N-048 §2.10, T-4880): the caps per
//! topic, per broker count and bytes in one statement, the window, and the sweep of a broker
//! that left `capture`.
//!
//! Runs when `JC_PORTAL_TEST_DATABASE_URL` points at a PostgreSQL the test may write to (locally:
//! `docker run -e POSTGRES_PASSWORD=… postgres:17-alpine`); otherwise it says so and returns.

use std::collections::BTreeSet;

use chrono::{DateTime, Duration, Utc};
use joinedcontext_portal::mqtt_capture::{CaptureStore, Message, BROKER_MESSAGES};

fn database_url() -> Option<String> {
    std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
}

/// A project nobody else's run writes to.
fn fresh_project() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("capture-{nanos}")
}

fn message(topic: &str, received_at: DateTime<Utc>, bytes: usize) -> Message {
    Message {
        topic: topic.to_owned(),
        client_id: None,
        received_at,
        expires_at: received_at + Duration::seconds(60),
        payload: vec![b'x'; bytes],
    }
}

#[tokio::test]
async fn the_caps_and_the_window_hold_in_postgres() {
    let Some(url) = database_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("the test database answers and migrates");
    let store = CaptureStore::new(Some(pool));
    let project = fresh_project();
    // Stored to the millisecond.
    let start =
        DateTime::from_timestamp_millis((Utc::now() - Duration::seconds(30)).timestamp_millis())
            .expect("an instant");
    let second = |s: i64| start + Duration::seconds(s);

    // Per topic: 5 of `a`, 2 of `b`, two kept per topic.
    let batch = (0..5)
        .map(|s| message("a", second(s), 1))
        .chain((0..2).map(|s| message("b", second(s), 1)))
        .collect();
    store
        .keep(&project, "cams", 2, batch, second(10))
        .await
        .expect("kept");
    let (kept, more) = store
        .read(&project, "cams", None, 10, second(10))
        .await
        .expect("read");
    let seen: Vec<(&str, DateTime<Utc>)> = kept
        .iter()
        .map(|m| (m.topic.as_str(), m.received_at))
        .collect();
    assert!(!more);
    assert_eq!(
        seen,
        [
            ("b", second(1)),
            ("b", second(0)),
            ("a", second(4)),
            ("a", second(3))
        ]
    );

    // Per broker: 1200 topics of one message, the newest 1000 kept.
    let batch = (0..1200)
        .map(|i| message(&format!("t/{i}"), second(11), 1))
        .collect();
    store
        .keep(&project, "count", 20, batch, second(12))
        .await
        .expect("kept");
    let (kept, more) = store
        .read(&project, "count", None, 1000, second(12))
        .await
        .expect("read");
    assert_eq!((kept.len(), more), (BROKER_MESSAGES, false));
    assert_eq!(kept.last().map(|m| m.topic.as_str()), Some("t/200"));

    // Per broker bytes: three of 3 MiB, the newest two kept, and the older small one with them
    // goes too.
    let mib = 1024 * 1024;
    let batch = vec![
        message("s", second(0), 1),
        message("a", second(1), 3 * mib),
        message("b", second(2), 3 * mib),
        message("c", second(3), 3 * mib),
    ];
    store
        .keep(&project, "bytes", 20, batch, second(4))
        .await
        .expect("kept");
    let (kept, _) = store
        .read(&project, "bytes", None, 10, second(4))
        .await
        .expect("read");
    let topics: Vec<&str> = kept.iter().map(|m| m.topic.as_str()).collect();
    assert_eq!(topics, ["c", "b"]);

    // The window: at 60 s the message of second 0 is past it, unread and then swept; a broker
    // that left capture loses everything.
    let (kept, _) = store
        .read(&project, "cams", None, 10, second(60))
        .await
        .expect("read");
    assert_eq!(kept.len(), 3, "b of second 0 expired");
    let capturing: BTreeSet<(String, String)> = [(project.clone(), "cams".to_owned())].into();
    store.sweep(&capturing, second(60)).await.expect("swept");
    for broker in ["count", "bytes"] {
        assert!(store
            .read(&project, broker, None, 10, second(0))
            .await
            .expect("read")
            .0
            .is_empty());
    }
    assert_eq!(store.purge(&project, "cams").await.expect("purged"), 3);
}
