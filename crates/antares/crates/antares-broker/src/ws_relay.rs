// SPDX-License-Identifier: EUPL-1.2
//! ADR-0026 rule 5 across pods: a WebSocket connection lives on the api pod
//! that upgraded it, while its notifications are matched and sent by
//! whichever notifier pod took the change. The notifier asks the holding pod
//! over core NATS request/reply on `antares.ws.<tenant>.<connection-id>`;
//! the holding pod answers from its hub (queued, overflow, too large,
//! unknown). The Tenant in the subject keeps one Tenant's requests off every
//! other Tenant's subscriptions (STO-15, NET-09, T-7275), and the hub checks
//! the notification's own Tenant against it.

use antares_bus::nats::NatsBus;
use antares_ws::{Hub, Relay};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::task::JoinHandle;

/// The subject prefix; the Tenant and the connection id follow, one token
/// each.
const SUBJECT: &str = "antares.ws.";

/// `antares.ws.<tenant>.<connection>`, or `None` when either part is not one
/// subject token of the names a Tenant (`[A-Za-z0-9_-]`) or a connection id
/// (lowercase base32) is made of: no `.`, `*` or `>` reaches a subject.
fn subject(tenant: &str, connection: &str) -> Option<String> {
    crate::relay_subject(SUBJECT, tenant, connection)
}

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
        tenant: &'a str,
        connection: &'a str,
        payload: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Option<Vec<u8>>> + Send + 'a>> {
        Box::pin(async move {
            self.bus
                .request(subject(tenant, connection)?, payload, WITHIN)
                .await
        })
    }

    fn listen(&self, hub: Weak<Hub>, tenant: &str, connection: &str) {
        let Some(subject) = subject(tenant, connection) else {
            tracing::warn!("a WebSocket connection's Tenant is no subject token: not relayed");
            return;
        };
        let (tenant, id) = (tenant.to_owned(), connection.to_owned());
        let task = self.bus.reply(subject, move |payload| {
            hub.upgrade().map(|h| h.answer(&tenant, &id, payload))
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

#[cfg(test)]
mod tests {
    use super::subject;

    /// T-7275: the Tenant is a token of its own, so two Tenants never share
    /// a subject and no wildcard or extra token gets in.
    #[test]
    fn the_subject_carries_the_tenant() {
        assert_eq!(
            subject("city-a", "abc234").as_deref(),
            Some("antares.ws.city-a.abc234")
        );
        assert_ne!(subject("city-a", "abc234"), subject("city-b", "abc234"));
        for (tenant, connection) in [
            ("a.b", "c"),
            ("*", "c"),
            (">", "c"),
            ("a", "c.d"),
            ("", "c"),
            ("a", ""),
        ] {
            assert_eq!(subject(tenant, connection), None, "{tenant} {connection}");
        }
    }
}
