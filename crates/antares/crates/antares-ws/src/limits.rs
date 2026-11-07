// SPDX-License-Identifier: EUPL-1.2
//! The bounds of ADR-0026 rule 10, rule 5's heartbeat and rule 7's grace,
//! read from the environment once and reported in `/q/health`.

use serde_json::{json, Value};
use std::time::Duration;

/// The largest notification one connection is sent: the writer's buffer is
/// bounded by it (threat F8), and a notification above it is a failed
/// delivery the Subscription records, never a partial write.
pub const MAX_NOTIFICATION_BYTES: usize = 1024 * 1024;

/// The variable of the bound of the same name (the broker knows it as a config key).
pub const QUEUE_ENV: &str = "ANTARES_WS_QUEUE";
/// The variable of the bound of the same name (the broker knows it as a config key).
pub const QUEUE_BYTES_ENV: &str = "ANTARES_WS_QUEUE_BYTES";
/// The variable of the bound of the same name (the broker knows it as a config key).
pub const RESUME_GRACE_ENV: &str = "ANTARES_WS_RESUME_GRACE_SECS";
/// The variable of the bound of the same name (the broker knows it as a config key).
pub const SUBSCRIPTION_LEASE_ENV: &str = "ANTARES_WS_SUBSCRIPTION_LEASE_SECS";
/// The variable of the bound of the same name (the broker knows it as a config key).
pub const MAX_LIFETIME_ENV: &str = "ANTARES_WS_MAX_LIFETIME_SECS";
/// The variable of the bound of the same name (the broker knows it as a config key).
pub const MAX_CONNECTIONS_ENV: &str = "ANTARES_WS_MAX_CONNECTIONS";
/// The variable of the bound of the same name (the broker knows it as a config key).
pub const MAX_CONNECTIONS_PER_TENANT_ENV: &str = "ANTARES_WS_MAX_CONNECTIONS_PER_TENANT";
/// The variable of the bound of the same name (the broker knows it as a config key).
pub const MAX_SUBSCRIPTIONS_ENV: &str = "ANTARES_WS_MAX_SUBSCRIPTIONS";
/// The variable of the bound of the same name (the broker knows it as a config key).
pub const ALLOWED_ORIGINS_ENV: &str = "ANTARES_WS_ALLOWED_ORIGINS";

/// Every variable this crate reads. The broker accepts these as config keys
/// from here, so a new one is added in this crate alone (T-3766).
pub const ENV_KEYS: &[&str] = &[
    QUEUE_ENV,
    QUEUE_BYTES_ENV,
    RESUME_GRACE_ENV,
    SUBSCRIPTION_LEASE_ENV,
    MAX_LIFETIME_ENV,
    MAX_CONNECTIONS_ENV,
    MAX_CONNECTIONS_PER_TENANT_ENV,
    MAX_SUBSCRIPTIONS_ENV,
    ALLOWED_ORIGINS_ENV,
];

/// Every bound a connection is held to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    /// `ANTARES_WS_QUEUE`: queued notifications per connection, and the
    /// length of its resume ring (W4).
    pub queue: usize,
    /// `ANTARES_WS_QUEUE_BYTES`: the bytes one connection's queue and resume
    /// ring hold together (F-ANT3-001); the ring's oldest leave first, and a
    /// queue past it overflows as one past `queue` does. At least
    /// [`MAX_NOTIFICATION_BYTES`].
    pub queue_bytes: usize,
    /// `ANTARES_WS_RESUME_GRACE_SECS`: how long a closed connection's
    /// Subscriptions and ring wait for a resume.
    pub resume_grace: Duration,
    /// `ANTARES_WS_SUBSCRIPTION_LEASE_SECS`: the most a Subscription made
    /// over a socket lives past its last renewal, so one a dead process
    /// held expires (rule 7a).
    pub subscription_lease: Duration,
    /// `ANTARES_WS_MAX_LIFETIME_SECS`: a socket ends at this age (W7).
    pub max_lifetime: Duration,
    /// `ANTARES_WS_MAX_CONNECTIONS` (W2).
    pub max_connections: usize,
    /// `ANTARES_WS_MAX_CONNECTIONS_PER_TENANT` (W2).
    pub max_connections_per_tenant: usize,
    /// `ANTARES_WS_MAX_SUBSCRIPTIONS`: Subscriptions per connection (W9).
    pub max_subscriptions: usize,
    /// `ANTARES_WS_ALLOWED_ORIGINS`: the browser `Origin`s an upgrade may
    /// carry; empty refuses every browser (W1).
    pub allowed_origins: Vec<String>,
    /// A connection with no client message and no Subscription this long
    /// is closed (W2).
    pub idle: Duration,
    /// How often a Ping is sent (RFC 6455 5.5.2).
    pub ping_every: Duration,
    /// A socket with no Pong this long is closed.
    pub pong_within: Duration,
    /// Frames a client may send a second on average; past
    /// `message_burst` ahead of that pace the socket closes with 1008 (W9).
    pub messages_per_sec: u32,
    /// Frames a client may send at once before `messages_per_sec` applies.
    pub message_burst: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            queue: 256,
            queue_bytes: 16 * 1024 * 1024,
            resume_grace: Duration::from_secs(30),
            subscription_lease: Duration::from_secs(300),
            max_lifetime: Duration::from_secs(300),
            max_connections: 10_000,
            max_connections_per_tenant: 1_000,
            max_subscriptions: 64,
            allowed_origins: Vec::new(),
            idle: Duration::from_secs(60),
            ping_every: Duration::from_secs(30),
            pong_within: Duration::from_secs(60),
            messages_per_sec: 20,
            message_burst: 100,
        }
    }
}

impl Limits {
    /// The limits the environment `var` names, each on its default when
    /// unset.
    ///
    /// ```
    /// let l = antares_ws::Limits::from_env(|k| (k == "ANTARES_WS_QUEUE").then(|| "8".to_owned())).unwrap();
    /// assert_eq!(l.queue, 8);
    /// ```
    ///
    /// # Errors
    /// The variable and its value when one is not a positive whole number:
    /// a bound the operator mistyped stops the broker rather than falling
    /// back to one the operator did not choose.
    pub fn from_env(var: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let d = Self::default();
        let set = |name: &str| -> Result<Option<u64>, String> {
            var(name).map_or(Ok(None), |v| match v.trim().parse::<u64>() {
                Ok(n) if n > 0 => Ok(Some(n)),
                _ => Err(format!("{name}={v:?} is not a positive whole number")),
            })
        };
        let count = |name: &str, default: usize| -> Result<usize, String> {
            Ok(set(name)?.map_or(default, |n| usize::try_from(n).unwrap_or(usize::MAX)))
        };
        let secs = |name: &str, default: Duration| -> Result<Duration, String> {
            Ok(set(name)?.map_or(default, Duration::from_secs))
        };
        let resume_grace = secs(RESUME_GRACE_ENV, d.resume_grace)?;
        let subscription_lease = secs(SUBSCRIPTION_LEASE_ENV, d.subscription_lease)?;
        // the last renewal is at most a third of the lease and one Ping old
        // when a socket closes; the grace must end before the lease does
        let floor = resume_grace.saturating_add(d.ping_every).saturating_mul(2);
        if subscription_lease <= floor {
            return Err(format!(
                "{SUBSCRIPTION_LEASE_ENV}={} must exceed {} (twice the resume grace plus the Ping interval)",
                subscription_lease.as_secs(),
                floor.as_secs()
            ));
        }
        let queue_bytes = count(QUEUE_BYTES_ENV, d.queue_bytes)?;
        if queue_bytes < MAX_NOTIFICATION_BYTES {
            return Err(format!(
                "{QUEUE_BYTES_ENV}={queue_bytes} must be at least {MAX_NOTIFICATION_BYTES} (one notification at its cap)"
            ));
        }
        Ok(Self {
            queue: count(QUEUE_ENV, d.queue)?,
            queue_bytes,
            resume_grace,
            subscription_lease,
            max_lifetime: secs(MAX_LIFETIME_ENV, d.max_lifetime)?,
            max_connections: count(MAX_CONNECTIONS_ENV, d.max_connections)?,
            max_connections_per_tenant: count(
                MAX_CONNECTIONS_PER_TENANT_ENV,
                d.max_connections_per_tenant,
            )?,
            max_subscriptions: count(MAX_SUBSCRIPTIONS_ENV, d.max_subscriptions)?,
            allowed_origins: var(ALLOWED_ORIGINS_ENV)
                .map(|v| {
                    v.split(',')
                        .map(str::trim)
                        .filter(|o| !o.is_empty())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            ..d
        })
    }

    /// What `/q/health` reports (rule 10: every limit beside the bounds).
    #[must_use]
    pub fn health(&self) -> Value {
        json!({
            "queue": self.queue,
            "queueBytes": self.queue_bytes,
            "resumeGraceSecs": self.resume_grace.as_secs(),
            "subscriptionLeaseSecs": self.subscription_lease.as_secs(),
            "maxLifetimeSecs": self.max_lifetime.as_secs(),
            "maxConnections": self.max_connections,
            "maxConnectionsPerTenant": self.max_connections_per_tenant,
            "maxSubscriptions": self.max_subscriptions,
            "allowedOrigins": self.allowed_origins.len(),
            "idleSecs": self.idle.as_secs(),
            "pingSecs": self.ping_every.as_secs(),
            "pongSecs": self.pong_within.as_secs(),
            "messagesPerSec": self.messages_per_sec,
            "messageBurst": self.message_burst,
            "maxMessageBytes": crate::envelope::MAX_MESSAGE_BYTES,
            "maxFrameBytes": crate::envelope::MAX_FRAME_BYTES,
            "maxNotificationBytes": MAX_NOTIFICATION_BYTES,
            "maxJsonDepth": antares_ops::bounds::MAX_JSON_DEPTH,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_is_the_default_and_a_mistyped_bound_stops_the_broker() -> Result<(), String> {
        assert_eq!(Limits::from_env(|_| None)?, Limits::default());
        let set = Limits::from_env(|k| match k {
            "ANTARES_WS_RESUME_GRACE_SECS" => Some("5".into()),
            "ANTARES_WS_ALLOWED_ORIGINS" => Some(" https://a.example , ,https://b.example".into()),
            _ => None,
        })?;
        assert_eq!(set.resume_grace, Duration::from_secs(5));
        assert_eq!(
            set.allowed_origins,
            ["https://a.example", "https://b.example"]
        );
        for bad in ["0", "-1", "x", ""] {
            let e = Limits::from_env(|k| (k == "ANTARES_WS_QUEUE").then(|| bad.to_owned()));
            assert!(e.is_err(), "{bad}");
        }
        assert_eq!(Limits::default().health().get("queue"), Some(&json!(256)));
        assert_eq!(
            Limits::default().health().get("queueBytes"),
            Some(&json!(16 * 1024 * 1024))
        );
        let bytes = Limits::from_env(|k| (k == QUEUE_BYTES_ENV).then(|| "2097152".to_owned()))?;
        assert_eq!(bytes.queue_bytes, 2_097_152);
        let tiny = Limits::from_env(|k| (k == QUEUE_BYTES_ENV).then(|| "1000".to_owned()));
        assert!(tiny.is_err(), "under one notification at its cap");
        // a lease the grace could outlast stops the broker
        let short = Limits::from_env(|k| (k == SUBSCRIPTION_LEASE_ENV).then(|| "120".to_owned()));
        assert!(short.is_err(), "{short:?}");
        let long = Limits::from_env(|k| match k {
            "ANTARES_WS_SUBSCRIPTION_LEASE_SECS" => Some("121".into()),
            _ => None,
        })?;
        assert_eq!(long.subscription_lease, Duration::from_secs(121));
        let graced = Limits::from_env(|k| (k == RESUME_GRACE_ENV).then(|| "300".to_owned()));
        assert!(
            graced.is_err(),
            "the default lease is shorter than this grace"
        );
        Ok(())
    }
}
