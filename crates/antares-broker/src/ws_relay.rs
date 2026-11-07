// SPDX-License-Identifier: EUPL-1.2
//! ADR-0026 rule 5 across pods: a WebSocket connection lives on the api pod
//! that upgraded it, while its notifications are matched and sent by
//! whichever notifier pod took the change. The notifier asks the holding pod
//! over core NATS request/reply on `antares.ws.<connection-id>`; the holding
//! pod answers from its hub (queued, overflow, too large, unknown).

use antares_bus::nats::NatsBus;
use antares_ws::{Hub, Relay};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::task::JoinHandle;

/// The subject prefix; a connection id is lowercase base32, one token.
const SUBJECT: &str = "antares.ws.";

/// How long a notifier waits for the holding pod. A pod that does not
/// answer within it is treated as not holding the connection: the delivery
/// fails like one to an unreachable HTTP endpoint (5.2.14.2 `failed`).
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
        connection: &'a str,
        payload: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Option<Vec<u8>>> + Send + 'a>> {
        Box::pin(
            self.bus
                .request(format!("{SUBJECT}{connection}"), payload, WITHIN),
        )
    }

    fn listen(&self, hub: Weak<Hub>, connection: &str) {
        let id = connection.to_owned();
        let task = self
            .bus
            .reply(format!("{SUBJECT}{connection}"), move |payload| {
                hub.upgrade().map(|h| h.answer(&id, payload))
            });
        if let Some(old) = self.listening().insert(connection.to_owned(), task) {
            old.abort();
        }
    }

    fn unlisten(&self, connection: &str) {
        if let Some(task) = self.listening().remove(connection) {
            task.abort();
        }
    }
}
