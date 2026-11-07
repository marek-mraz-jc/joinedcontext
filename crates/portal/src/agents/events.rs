//! Server-Sent Events broadcasting and stream subscriptions (AG-45).

use crate::agents::run::{AgentRunEvent, AgentRunStatus};
use crate::agents::store::AgentStore;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

/// One channel per run someone listens to. A channel goes when nobody listens any more or when
/// its run ends, so the map holds the runs being watched, not every run since the start.
#[derive(Clone, Default)]
pub struct AgentEventHub {
    channels: Arc<RwLock<HashMap<String, broadcast::Sender<AgentRunEvent>>>>,
}

/// A `status` event that ends its run: nothing follows it on the run's channel.
fn ends_the_run(event: &AgentRunEvent) -> bool {
    event.kind == "status"
        && event
            .payload
            .get("status")
            .and_then(serde_json::Value::as_str)
            .and_then(AgentRunStatus::parse)
            .is_some_and(|status| status.is_terminal())
}

impl AgentEventHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Hands `event` to the run's listeners. A run nobody listens to gets no channel: its
    /// events are in the store, which a stream reads before it goes live.
    pub async fn broadcast(&self, event: &AgentRunEvent) {
        let mut map = self.channels.write().await;
        if let Some(sender) = map.get(&event.run_id) {
            // The receivers keep what was sent before the sender goes, then read the close.
            let _ = sender.send(event.clone());
            if ends_the_run(event) || sender.receiver_count() == 0 {
                map.remove(&event.run_id);
            }
        }
    }

    pub async fn subscribe(&self, run_id: &str) -> broadcast::Receiver<AgentRunEvent> {
        let mut map = self.channels.write().await;
        map.retain(|_, sender| sender.receiver_count() > 0);
        let sender = map.entry(run_id.to_string()).or_insert_with(|| {
            let (tx, _) = broadcast::channel(128);
            tx
        });
        sender.subscribe()
    }

    #[cfg(test)]
    async fn channels(&self) -> usize {
        self.channels.read().await.len()
    }
}

/// One run's events in order, for the driver that answers them. The live channel holds 128
/// events; a driver busy with a long tool call can fall further behind, and the events it missed
/// (a person's answer among them) are read back from the store instead of being dropped (JC1-31).
pub struct RunInbox {
    live: broadcast::Receiver<AgentRunEvent>,
    run_id: String,
    /// The newest sequence number handed out: what the store is read after, and what a live
    /// event at or below it already came from the store.
    seen: i64,
    backlog: VecDeque<AgentRunEvent>,
}

impl RunInbox {
    /// `live` subscribed after the run's events up to `seen` were stored.
    pub fn new(live: broadcast::Receiver<AgentRunEvent>, run_id: &str, seen: i64) -> Self {
        Self {
            live,
            run_id: run_id.to_owned(),
            seen,
            backlog: VecDeque::new(),
        }
    }

    /// The next event, or `None` once the channel closed.
    pub async fn recv(&mut self, store: &AgentStore) -> Option<AgentRunEvent> {
        loop {
            if let Some(event) = self.backlog.pop_front() {
                self.seen = event.seq;
                return Some(event);
            }
            match self.live.recv().await {
                Ok(event) if event.seq <= self.seen => {}
                Ok(event) => {
                    self.seen = event.seq;
                    return Some(event);
                }
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    match store.events_since(&self.run_id, self.seen).await {
                        Ok(events) => self.backlog.extend(events),
                        Err(err) => tracing::warn!(
                            run_id = %self.run_id, missed, error = %err,
                            "a lagging run could not read the events it missed"
                        ),
                    }
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(run: &str, kind: &str, status: &str) -> AgentRunEvent {
        AgentRunEvent {
            run_id: run.into(),
            seq: 1,
            kind: kind.into(),
            payload: json!({ "status": status }),
            created_at: String::new(),
        }
    }

    #[tokio::test]
    async fn a_run_that_ends_leaves_no_channel_and_its_listener_reads_the_end() {
        let hub = AgentEventHub::new();
        let mut live = hub.subscribe("r1").await;
        hub.broadcast(&event("r1", "status", "building")).await;
        assert_eq!(hub.channels().await, 1);
        hub.broadcast(&event("r1", "status", "published")).await;
        assert_eq!(hub.channels().await, 0);
        assert_eq!(
            live.recv().await.expect("building").payload["status"],
            "building"
        );
        assert_eq!(
            live.recv().await.expect("published").payload["status"],
            "published"
        );
        assert!(
            live.recv().await.is_err(),
            "the channel closes after the end"
        );
    }

    #[tokio::test]
    async fn runs_nobody_listens_to_hold_no_channel() {
        let hub = AgentEventHub::new();
        for run in 0..50 {
            hub.broadcast(&event(&format!("r{run}"), "message", ""))
                .await;
        }
        assert_eq!(hub.channels().await, 0);
        drop(hub.subscribe("gone").await);
        let _kept = hub.subscribe("watched").await;
        assert_eq!(hub.channels().await, 1, "a dropped listener's channel goes");
    }

    #[tokio::test]
    async fn a_lagging_inbox_reads_what_it_missed_from_the_store_in_order() {
        let hub = AgentEventHub::new();
        let store = AgentStore::new(None);
        store
            .append_event("r1", "message", json!({ "text": "before" }))
            .await
            .expect("stored");
        let mut inbox = RunInbox::new(hub.subscribe("r1").await, "r1", 1);
        for n in 0..200 {
            let event = store
                .append_event("r1", "answer", json!({ "n": n }))
                .await
                .expect("stored");
            hub.broadcast(&event).await;
        }
        let mut read = Vec::new();
        for _ in 0..200 {
            let event = tokio::time::timeout(std::time::Duration::from_secs(5), inbox.recv(&store))
                .await
                .expect("no event lost")
                .expect("an event");
            read.push(event.payload["n"].as_i64().expect("n"));
        }
        assert_eq!(read, (0..200).collect::<Vec<_>>(), "none lost, none twice");
    }
}
