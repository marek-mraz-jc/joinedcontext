// SPDX-License-Identifier: EUPL-1.2
//! Resource subscriptions (MCP section 5, D-7; MC7, MC17): a
//! `subscriptions/listen` stream watches Entity resources of one view. Each
//! URI is backed by an internal NGSI-LD Subscription made through the view
//! as the caller (ADR-0023 rule 9), whose endpoint is this process's
//! [`Sink`]; a notification becomes `notifications/resources/updated` with
//! the URI only, and the client reads the Entity again through the gate.
//! Where several replicas deliver, the one that took the change asks the
//! one holding the stream through its [`Relay`] (S10-19).

use antares_ops::{DeliveryError, DeliveryFuture, NgsiError, NotificationSink, Outbound};
use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, LazyLock, Mutex, OnceLock, PoisonError};
use std::time::Duration;
use tokio::sync::Notify;

/// The scheme of an internal Subscription's endpoint: `x-antares-mcp:<stream>`.
pub const SCHEME: &str = "x-antares-mcp";

/// The most Entity URIs one stream watches; the rest are left out of the
/// acknowledgment (MCP section 8).
pub const MAX_URIS: usize = 64;

/// How long a stream lives; its Subscriptions expire with it, so a process
/// that dies leaves nothing behind for longer (MCP 5.2).
pub const LIFETIME: Duration = Duration::from_secs(3600);

/// How often an open stream asks again whether its caller may still read
/// through the view, besides before each event; a caller who may not is
/// closed out (NET-08).
pub const RECHECK: Duration = Duration::from_secs(60);

/// The bounds on open streams in one process (MCP section 8).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Caps {
    /// Per subject and Tenant: callers that send no subject header share it.
    pub subject: usize,
    /// Per Tenant.
    pub tenant: usize,
    /// In the process.
    pub process: usize,
}

/// The caps a broker runs with.
pub(crate) const CAPS: Caps = Caps {
    subject: 4,
    tenant: 64,
    process: 256,
};

tokio::task_local! {
    /// The stream a Subscription is being made for: the sink accepts its
    /// endpoint only inside this scope (MCP 5.3, "no client can write into
    /// a stream").
    static CREATING: String;
}

/// Run `f` as the making of Subscriptions for `stream`.
pub(crate) async fn creating<F: std::future::Future>(stream: &str, f: F) -> F::Output {
    CREATING.scope(stream.to_owned(), f).await
}

/// What a [`Relay`] returns.
pub type RelayFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What the replica holding a stream answers a relayed change with; `None`
/// once it holds no streams at all.
pub type Answer = Box<dyn Fn(&[u8]) -> Option<Vec<u8>> + Send + Sync>;

/// The way to a stream another replica holds (NATS request/reply in the
/// broker). Each request and listen names the stream's Tenant.
pub trait Relay: Send + Sync {
    /// Ask whoever holds `stream` of `tenant`; `None` when nobody answers.
    fn request<'a>(
        &'a self,
        tenant: &'a str,
        stream: &'a str,
        payload: Vec<u8>,
    ) -> RelayFuture<'a, Option<Vec<u8>>>;

    /// Answer for `stream` of `tenant` by `answer`; resolved once a request
    /// sent from then on is heard.
    fn listen<'a>(
        &'a self,
        tenant: &'a str,
        stream: &'a str,
        answer: Answer,
    ) -> RelayFuture<'a, ()>;

    /// Answer for `stream` no longer.
    fn unlisten(&self, stream: &str);
}

/// A relayed change the holder flagged; anything else is none.
const FLAGGED: &[u8] = b"1";

/// One open stream.
struct Stream {
    owner: (String, Vec<(String, Vec<u8>)>),
    /// Its internal Subscriptions, by id, and the URI each watches.
    watching: HashMap<String, String>,
    /// URIs changed since the stream last sent: one flag each, so a burst
    /// is one event and the queue is bounded by the URI count.
    pending: BTreeSet<String>,
    wake: Arc<Notify>,
}

/// Every open stream of this process, by id.
#[derive(Default)]
pub(crate) struct Streams {
    open: Mutex<HashMap<String, Stream>>,
    relay: OnceLock<Arc<dyn Relay>>,
}

/// This process's streams.
pub(crate) fn streams() -> &'static Arc<Streams> {
    static STREAMS: LazyLock<Arc<Streams>> = LazyLock::new(Arc::default);
    &STREAMS
}

/// Deliver to the streams of other replicas through `relay`, and answer
/// theirs for this one's: a broker calls it once at start, where several
/// may deliver a notification. The first relay stays.
pub fn set_relay(relay: Arc<dyn Relay>) {
    streams().use_relay(relay);
}

impl Streams {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Stream>> {
        // a holder that panicked left the map whole: each change is one step
        self.open.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Open a stream for `subject` in `tenant` within `caps`: its id and
    /// what wakes it, or which cap it hit.
    pub(crate) fn open(
        &self,
        tenant: &str,
        subject: Vec<(String, Vec<u8>)>,
        caps: Caps,
    ) -> Result<(String, Arc<Notify>), &'static str> {
        let owner = (tenant.to_owned(), subject);
        let mut all = self.lock();
        if all.len() >= caps.process {
            return Err("this process holds as many listen streams as it serves");
        }
        if all.values().filter(|s| s.owner.0 == owner.0).count() >= caps.tenant {
            return Err("this Tenant holds as many listen streams as it may");
        }
        if all.values().filter(|s| s.owner == owner).count() >= caps.subject {
            return Err("this caller holds as many listen streams as it may");
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        let wake = Arc::new(Notify::new());
        all.insert(
            id.clone(),
            Stream {
                owner,
                watching: HashMap::new(),
                pending: BTreeSet::new(),
                wake: wake.clone(),
            },
        );
        Ok((id, wake))
    }

    /// Record that Subscription `sub` of `stream` watches `uri`.
    pub(crate) fn watch(&self, stream: &str, sub: &str, uri: &str) {
        if let Some(s) = self.lock().get_mut(stream) {
            s.watching.insert(sub.to_owned(), uri.to_owned());
        }
    }

    /// Forget Subscription `sub` of `stream`.
    pub(crate) fn unwatch(&self, stream: &str, sub: &str) {
        if let Some(s) = self.lock().get_mut(stream) {
            s.watching.remove(sub);
        }
    }

    fn use_relay(&self, relay: Arc<dyn Relay>) {
        let _first_wins = self.relay.set(relay);
    }

    /// Answer the other replicas for `stream` of `tenant`, if they deliver.
    pub(crate) async fn relayed(self: &Arc<Self>, stream: &str, tenant: &str) {
        let Some(relay) = self.relay.get() else {
            return;
        };
        let (here, id, of) = (Arc::downgrade(self), stream.to_owned(), tenant.to_owned());
        let answer: Answer = Box::new(move |sub| {
            let here = here.upgrade()?;
            let flagged = std::str::from_utf8(sub).is_ok_and(|sub| {
                // the subject named this Tenant; the stream must be its own
                here.held_by(&id, &of) && here.changed(&id, sub)
            });
            Some(if flagged {
                FLAGGED.to_vec()
            } else {
                b"0".to_vec()
            })
        });
        relay.listen(tenant, stream, answer).await;
    }

    fn held_by(&self, stream: &str, tenant: &str) -> bool {
        self.lock().get(stream).is_some_and(|s| s.owner.0 == tenant)
    }

    /// Deliver a change of `sub` in `tenant` to `stream`, here or on the
    /// replica that holds it.
    pub(crate) async fn reach(&self, tenant: &str, stream: &str, sub: &str) -> bool {
        if self.held_by(stream, tenant) {
            return self.changed(stream, sub);
        }
        let Some(relay) = self.relay.get() else {
            return false;
        };
        let reply = relay.request(tenant, stream, sub.as_bytes().to_vec()).await;
        reply.as_deref() == Some(FLAGGED)
    }

    /// A notification of Subscription `sub` for `stream`: flag its URI and
    /// wake the stream. `false` when the stream is not open here or did
    /// not make `sub`.
    pub(crate) fn changed(&self, stream: &str, sub: &str) -> bool {
        let mut all = self.lock();
        let Some(s) = all.get_mut(stream) else {
            return false;
        };
        let Some(uri) = s.watching.get(sub).cloned() else {
            return false;
        };
        s.pending.insert(uri);
        s.wake.notify_one();
        true
    }

    /// The URIs flagged since the last call, cleared.
    pub(crate) fn take(&self, stream: &str) -> Vec<String> {
        self.lock()
            .get_mut(stream)
            .map(|s| std::mem::take(&mut s.pending).into_iter().collect())
            .unwrap_or_default()
    }

    /// Close `stream`: the ids of the Subscriptions it made.
    pub(crate) fn close(&self, stream: &str) -> Vec<String> {
        if let Some(relay) = self.relay.get() {
            relay.unlisten(stream);
        }
        self.lock()
            .remove(stream)
            .map(|s| s.watching.into_keys().collect())
            .unwrap_or_default()
    }
}

/// The in-process notification binding of the MCP surface: scheme
/// `x-antares-mcp:`, no socket. A broker built with feature `mcp` registers it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Sink;

impl NotificationSink for Sink {
    fn schemes(&self) -> &'static [&'static str] {
        &[SCHEME]
    }

    /// Only the endpoint of the stream making the Subscription, while it
    /// makes it: the same URI over HTTP, a tool, or another stream is
    /// `BadRequestData` (MCP 5.3).
    fn parse_endpoint(&self, uri: &str, _notifier_info: &[(&str, &str)]) -> Result<(), NgsiError> {
        let id = uri.strip_prefix(SCHEME).and_then(|r| r.strip_prefix(':'));
        let own = id.is_some_and(|id| CREATING.try_with(|c| c == id).unwrap_or(false));
        if own {
            Ok(())
        } else {
            Err(NgsiError::BadRequestData(format!(
                "a {SCHEME} endpoint is set by its own MCP stream, never named"
            )))
        }
    }

    /// It flags a URI of a stream and opens no socket of its own.
    fn network(&self) -> bool {
        false
    }

    fn deliver<'a>(
        &'a self,
        uri: &'a str,
        out: &'a Outbound,
        _timeout: Duration,
    ) -> DeliveryFuture<'a> {
        let stream = uri.strip_prefix(SCHEME).and_then(|r| r.strip_prefix(':'));
        let sub = out.body.get("subscriptionId").and_then(|v| v.as_str());
        Box::pin(async move {
            let delivered = match (stream, sub) {
                (Some(stream), Some(sub)) => streams().reach(tenant_of(out), stream, sub).await,
                _ => false,
            };
            if delivered {
                Ok(())
            } else {
                Err(DeliveryError::failed("no such MCP stream"))
            }
        })
    }
}

/// The Tenant the broker marked `out` with, read as WebSocket reads it.
fn tenant_of(out: &Outbound) -> &str {
    out.receiver_info
        .iter()
        .rev()
        .find(|(k, _)| k.eq_ignore_ascii_case("NGSILD-Tenant"))
        .map_or("default", |(_, v)| v.as_str())
}

/// `now` plus `after`, as an NGSI-LD `DateTime` (UTC, seconds).
pub(crate) fn date_time_after(now: std::time::SystemTime, after: Duration) -> String {
    let secs = now
        .checked_add(after)
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // days since 1970-01-01 to a civil date (H. Hinnant, `civil_from_days`);
    // every step stays far inside u64 for any time a SystemTime holds
    let z = days.saturating_add(719_468);
    let (era, doe) = (z / 146_097, z % 146_097);
    let yoe = doe
        .saturating_sub(doe / 1460)
        .saturating_add(doe / 36_524)
        .saturating_sub(doe / 146_096)
        / 365;
    let doy = doe.saturating_sub(
        yoe.saturating_mul(365)
            .saturating_add(yoe / 4)
            .saturating_sub(yoe / 100),
    );
    let mp = doy.saturating_mul(5).saturating_add(2) / 153;
    let day = doy
        .saturating_sub(mp.saturating_mul(153).saturating_add(2) / 5)
        .saturating_add(1);
    let month = if mp < 10 {
        mp.saturating_add(3)
    } else {
        mp.saturating_sub(9)
    };
    let year = yoe
        .saturating_add(era.saturating_mul(400))
        .saturating_add(u64::from(month <= 2));
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL: Caps = Caps {
        subject: 1,
        tenant: 2,
        process: 3,
    };

    fn who(n: u8) -> Vec<(String, Vec<u8>)> {
        vec![("x-user".into(), vec![n])]
    }

    #[test]
    fn streams_are_capped_per_subject_tenant_and_process() {
        let s = Streams::default();
        assert!(s.open("a", who(1), SMALL).is_ok());
        assert_eq!(
            s.open("a", who(1), SMALL).map(|_| ()),
            Err("this caller holds as many listen streams as it may")
        );
        assert!(s.open("a", who(2), SMALL).is_ok());
        assert_eq!(
            s.open("a", who(3), SMALL).map(|_| ()),
            Err("this Tenant holds as many listen streams as it may")
        );
        let (id, _) = s.open("b", who(1), SMALL).unwrap_or_default();
        assert_eq!(
            s.open("c", who(1), SMALL).map(|_| ()),
            Err("this process holds as many listen streams as it serves")
        );
        assert_eq!(s.close(&id), Vec::<String>::new());
        assert!(s.open("c", who(1), SMALL).is_ok());
    }

    #[test]
    fn a_change_flags_its_uri_once_and_only_for_the_stream_that_made_it() {
        let s = Streams::default();
        let (id, _) = s.open("a", who(1), SMALL).unwrap_or_default();
        s.watch(&id, "sub-1", "ngsi-ld://v/entities/x");
        assert!(s.changed(&id, "sub-1"));
        assert!(s.changed(&id, "sub-1"));
        assert!(!s.changed(&id, "sub-2"));
        assert!(!s.changed("other", "sub-1"));
        assert_eq!(s.take(&id), vec!["ngsi-ld://v/entities/x".to_owned()]);
        assert_eq!(s.take(&id), Vec::<String>::new());
        s.unwatch(&id, "sub-1");
        assert!(!s.changed(&id, "sub-1"));
        s.watch(&id, "sub-3", "ngsi-ld://v/entities/y");
        assert_eq!(s.close(&id), vec!["sub-3".to_owned()]);
        assert_eq!(s.take(&id), Vec::<String>::new());
        s.watch(&id, "sub-4", "u");
        s.unwatch(&id, "sub-4");
    }

    /// Request/reply between the replicas of one test.
    #[derive(Default)]
    struct Wire(Mutex<HashMap<(String, String), Answer>>);

    impl Wire {
        fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<(String, String), Answer>> {
            self.0.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    impl Relay for Wire {
        fn request<'a>(
            &'a self,
            tenant: &'a str,
            stream: &'a str,
            payload: Vec<u8>,
        ) -> RelayFuture<'a, Option<Vec<u8>>> {
            let key = (tenant.to_owned(), stream.to_owned());
            let reply = self.lock().get(&key).and_then(|answer| answer(&payload));
            Box::pin(std::future::ready(reply))
        }

        fn listen<'a>(
            &'a self,
            tenant: &'a str,
            stream: &'a str,
            answer: Answer,
        ) -> RelayFuture<'a, ()> {
            let key = (tenant.to_owned(), stream.to_owned());
            self.lock().insert(key, answer);
            Box::pin(std::future::ready(()))
        }

        fn unlisten(&self, stream: &str) {
            self.lock().retain(|(_, s), _| s != stream);
        }
    }

    /// T-7507 (S10-19): a change delivered by one replica reaches the stream
    /// another holds, of that Tenant alone and only while it is open.
    #[tokio::test]
    async fn a_change_on_one_replica_reaches_the_stream_of_another() {
        let (holds, delivers) = (Arc::new(Streams::default()), Arc::new(Streams::default()));
        let wire = Arc::new(Wire::default());
        holds.use_relay(wire.clone());
        delivers.use_relay(wire.clone());
        let (id, _) = holds.open("a", who(1), SMALL).unwrap_or_default();
        holds.relayed(&id, "a").await;
        holds.watch(&id, "sub-1", "ngsi-ld://v/entities/x");
        assert!(delivers.reach("a", &id, "sub-1").await);
        assert_eq!(holds.take(&id), vec!["ngsi-ld://v/entities/x".to_owned()]);
        assert!(!delivers.reach("a", &id, "sub-2").await);
        assert!(!delivers.reach("b", &id, "sub-1").await);
        assert!(!holds.reach("b", &id, "sub-1").await);
        // a request on another Tenant's subject for this stream flags nothing
        holds.relayed(&id, "b").await;
        assert!(!delivers.reach("b", &id, "sub-1").await);
        holds.close(&id);
        assert!(!delivers.reach("a", &id, "sub-1").await);
        assert!(wire.lock().is_empty());
    }

    #[tokio::test]
    async fn the_sink_takes_only_its_own_stream_while_it_makes_it() {
        let uri = format!("{SCHEME}:abc");
        assert!(Sink.parse_endpoint(&uri, &[]).is_err());
        assert!(creating("abc", async { Sink.parse_endpoint(&uri, &[]) })
            .await
            .is_ok());
        assert!(creating("abd", async { Sink.parse_endpoint(&uri, &[]) })
            .await
            .is_err());
        let other = "urn:antares:ws:abc";
        assert!(creating("abc", async { Sink.parse_endpoint(other, &[]) })
            .await
            .is_err());
        assert_eq!(Sink.schemes(), &[SCHEME]);
        assert!(!Sink.network());
    }

    #[tokio::test]
    async fn the_sink_wakes_the_stream_that_made_the_subscription() {
        let (id, _) = streams().open("t", who(9), CAPS).unwrap_or_default();
        streams().watch(&id, "urn:ngsi-ld:Subscription:s", "ngsi-ld://v/entities/x");
        let out = |sub: serde_json::Value| Outbound {
            body: serde_json::json!({"subscriptionId": sub}),
            accept: "application/json".into(),
            link: String::new(),
            receiver_info: vec![("NGSILD-Tenant".into(), "t".into())],
            notifier_info: Vec::new(),
        };
        let mine = out(serde_json::json!("urn:ngsi-ld:Subscription:s"));
        let endpoint = format!("{SCHEME}:{id}");
        let second = Duration::from_secs(1);
        assert!(Sink.deliver(&endpoint, &mine, second).await.is_ok());
        let theirs = out(serde_json::json!("urn:ngsi-ld:Subscription:t"));
        assert!(Sink.deliver(&endpoint, &theirs, second).await.is_err());
        assert!(Sink
            .deliver(&endpoint, &out(serde_json::Value::Null), second)
            .await
            .is_err());
        assert!(Sink
            .deliver("x-antares-mcp-other", &mine, second)
            .await
            .is_err());
        assert_eq!(
            streams().take(&id),
            vec!["ngsi-ld://v/entities/x".to_owned()]
        );
        streams().close(&id);
        assert!(Sink.deliver(&endpoint, &mine, second).await.is_err());
    }

    #[test]
    fn a_date_time_is_utc_seconds() {
        let at = |s: u64| std::time::UNIX_EPOCH.checked_add(Duration::from_secs(s));
        let day = |s| at(s).map(|t| date_time_after(t, Duration::ZERO));
        assert_eq!(day(0).as_deref(), Some("1970-01-01T00:00:00Z"));
        assert_eq!(day(951_782_400).as_deref(), Some("2000-02-29T00:00:00Z"));
        assert_eq!(day(1_791_684_245).as_deref(), Some("2026-10-11T02:04:05Z"));
        assert_eq!(day(4_107_542_399).as_deref(), Some("2100-02-28T23:59:59Z"));
        let later = at(0).map(|t| date_time_after(t, LIFETIME));
        assert_eq!(later.as_deref(), Some("1970-01-01T01:00:00Z"));
    }
}
