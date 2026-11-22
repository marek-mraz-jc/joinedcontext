// SPDX-License-Identifier: EUPL-1.2
//! MCP resource streams across pods (T-7507): a `subscriptions/listen`
//! stream lives on the pod that opened it, its changes are sent by whichever
//! notifier pod took them. The notifier asks the holding pod over core NATS
//! request/reply on `antares.mcp.<tenant>.<stream>`, as `ws_relay` does for
//! a WebSocket; the holder checks the stream's own Tenant too.

use antares_bus::nats::NatsBus;
use antares_mcp::{Answer, Relay, RelayFuture};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::task::JoinHandle;

/// The subject prefix; the Tenant and the stream id follow, one token each.
const SUBJECT: &str = "antares.mcp.";

/// How long a notifier waits for the holding pod; past it the delivery
/// fails like one to an unreachable endpoint.
const WITHIN: Duration = Duration::from_secs(2);

/// The relay over the broker's NATS connection.
pub struct NatsRelay {
    bus: Arc<NatsBus>,
    listening: Mutex<HashMap<String, JoinHandle<()>>>,
}

impl NatsRelay {
    pub fn new(bus: Arc<NatsBus>) -> Self {
        Self {
            bus,
            listening: Mutex::default(),
        }
    }

    fn listening(&self) -> std::sync::MutexGuard<'_, HashMap<String, JoinHandle<()>>> {
        self.listening
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Relay for NatsRelay {
    fn request<'a>(
        &'a self,
        tenant: &'a str,
        stream: &'a str,
        payload: Vec<u8>,
    ) -> RelayFuture<'a, Option<Vec<u8>>> {
        Box::pin(async move {
            let subject = crate::relay_subject(SUBJECT, tenant, stream)?;
            self.bus.request(subject, payload, WITHIN).await
        })
    }

    fn listen<'a>(
        &'a self,
        tenant: &'a str,
        stream: &'a str,
        answer: Answer,
    ) -> RelayFuture<'a, ()> {
        Box::pin(async move {
            let Some(subject) = crate::relay_subject(SUBJECT, tenant, stream) else {
                tracing::warn!("an MCP stream's Tenant is no subject token: not relayed");
                return;
            };
            match self.bus.reply_ready(subject, answer).await {
                Ok(task) => {
                    if let Some(old) = self.listening().insert(stream.to_owned(), task) {
                        old.abort();
                    }
                }
                Err(e) => tracing::warn!("an MCP stream is not relayed: {e}"),
            }
        })
    }

    fn unlisten(&self, stream: &str) {
        if let Some(task) = self.listening().remove(stream) {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-7507 (S10-19): asked the moment `listen` resolves, answered on the
    /// stream's own Tenant alone, and no longer after `unlisten`.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_change_reaches_the_holder_over_nats_in_its_tenant_alone() {
        let Ok(url) = std::env::var("ANTARES_TEST_NATS_URL") else {
            eprintln!("SKIP: ANTARES_TEST_NATS_URL not set");
            return;
        };
        let pod = || async { NatsRelay::new(Arc::new(NatsBus::connect(&url).await.expect("bus"))) };
        let (holds, delivers) = (pod().await, pod().await);
        let stream = format!("{:?}", std::time::SystemTime::now())
            .replace(|c: char| !c.is_ascii_alphanumeric(), "");
        let echo: Answer = Box::new(|sub| Some(sub.to_vec()));
        holds.listen("city-a", &stream, echo).await;
        let ask = |tenant| delivers.request(tenant, &stream, b"sub".to_vec());
        assert_eq!(ask("city-a").await.as_deref(), Some(b"sub".as_slice()));
        assert_eq!(ask("city-b").await, None);
        assert_eq!(ask("city-a.>").await, None);
        holds.unlisten(&stream);
        tokio::task::yield_now().await;
        let mut gone = false;
        for _ in 0..50 {
            gone = ask("city-a").await.is_none();
            if gone {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(gone);
    }
}
