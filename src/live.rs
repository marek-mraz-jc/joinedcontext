//! Live updates of a space's entities (API/01 §32, ADR-N-042 §3.4, T-3105).
//!
//! An open data view watches one space and type. The first watcher of a pair has the Portal write
//! one NGSI-LD subscription through the space surface with its own ServiceAccount, delivering to
//! `/live-notify/{key}` on the Portal's public host. The notification is read under the Portal's
//! ServiceAccount, whose grants are not the person's, so the hub passes on only that the type
//! changed: no entity id, no attribute name, no value (T-4876). A window then reads the rows again
//! with the person's session, and the gateway narrows that read to the person's row and attribute
//! grants; a notification, forged or not, shows nobody anything their grants do not.
//!
//! Channels and delivery keys belong to one (project, space, type): two projects with a space of
//! the same name never hear each other.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hmac::{Hmac, Mac};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;

use crate::reconciler::subscriptions::SubscriptionSync;

/// How long a written subscription lives in the broker.
pub const LIFETIME: Duration = Duration::from_secs(24 * 3600);
/// A watcher after this long writes the subscription again, before it expires.
pub const RENEW_AFTER: Duration = Duration::from_secs(12 * 3600);
/// How many changes a slow window may fall behind before it misses some and reads again.
const BUFFER: usize = 256;

/// That entities of a type changed. Which ones, and which attributes, the window learns from its
/// own read, as far as the person's grants reach (T-4876).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Changed {
    #[serde(rename = "type")]
    pub entity_type: String,
}

/// (project, space, type).
type Pair = (String, String, String);

pub struct LiveHub {
    secret: Vec<u8>,
    /// The Portal's public origin, where the gateway delivers.
    public_base: String,
    channels: Mutex<HashMap<Pair, broadcast::Sender<Arc<Changed>>>>,
    /// The delivery key of every pair a window watches, to the pair: one lookup per delivery,
    /// then the tag verified in constant time (F-POR4-021).
    keys: Mutex<HashMap<String, Pair>>,
    written: Mutex<HashMap<Pair, Instant>>,
    writer: Option<Arc<SubscriptionSync>>,
}

impl LiveHub {
    /// A hub writing subscriptions with `writer`; without one the windows hear nothing, and say so
    /// nowhere because there is nothing wrong with the data.
    pub fn new(public_base: &str, writer: Option<Arc<SubscriptionSync>>) -> Self {
        // The CSPRNG the CSRF token and the OIDC state are minted with.
        let secret = crate::auth::csrf::new_token().into_bytes();
        Self {
            secret,
            public_base: public_base.trim_end_matches('/').to_owned(),
            channels: Mutex::new(HashMap::new()),
            keys: Mutex::new(HashMap::new()),
            written: Mutex::new(HashMap::new()),
            writer,
        }
    }

    fn mac(&self, project: &str, space: &str, entity_type: &str) -> Hmac<Sha256> {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.secret).expect("any key length");
        for part in [project, space] {
            mac.update(part.as_bytes());
            mac.update(&[0]);
        }
        mac.update(entity_type.as_bytes());
        mac
    }

    /// The delivery key of one pair: an HMAC only this process can make.
    pub fn key(&self, project: &str, space: &str, entity_type: &str) -> String {
        hex(&self
            .mac(project, space, entity_type)
            .finalize()
            .into_bytes())
    }

    /// The watched pair `key` was made for, its tag verified in constant time.
    fn pair_of(&self, key: &str) -> Option<Pair> {
        let tag = unhex(key)?;
        let pair = self
            .keys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key.to_ascii_lowercase())
            .cloned()?;
        self.mac(&pair.0, &pair.1, &pair.2)
            .verify_slice(&tag)
            .is_ok()
            .then_some(pair)
    }

    /// Forgets every pair no window listens to any more, so a hub that served many spaces holds
    /// only what is watched now (F-POR4-021).
    fn prune(&self) {
        let mut channels = self.channels.lock().unwrap_or_else(|e| e.into_inner());
        channels.retain(|_, sender| sender.receiver_count() > 0);
        self.keys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|_, pair| channels.contains_key(pair));
        // A pair watched again later writes its subscription again (F-R2POR4-015).
        self.written
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|pair, _| channels.contains_key(pair));
    }

    /// A window's stream of one pair's changes; the subscription is written when it is new or old,
    /// into the tenant of the space's URN `segment` (PF-84, F-POR4-012).
    pub fn watch(
        self: &Arc<Self>,
        project: &str,
        space: &str,
        segment: &str,
        entity_type: &str,
    ) -> broadcast::Receiver<Arc<Changed>> {
        self.prune();
        let pair = (project.to_owned(), space.to_owned(), entity_type.to_owned());
        self.keys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(self.key(project, space, entity_type), pair.clone());
        let receiver = self
            .channels
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(pair.clone())
            .or_insert_with(|| broadcast::channel(BUFFER).0)
            .subscribe();
        let due = {
            let mut written = self.written.lock().unwrap_or_else(|e| e.into_inner());
            let due = written
                .get(&pair)
                .is_none_or(|at| at.elapsed() >= RENEW_AFTER);
            if due {
                written.insert(pair.clone(), Instant::now());
            }
            due
        };
        if due {
            let hub = Arc::clone(self);
            let segment = segment.to_owned();
            tokio::spawn(async move {
                if let Err(reason) = hub.write(&pair.0, &segment, &pair.1, &pair.2).await {
                    tracing::warn!(project = %pair.0, space = %pair.1, r#type = %pair.2, %reason, "live-update subscription not written");
                    // The next window tries again rather than waiting half a day.
                    hub.written
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .remove(&pair);
                }
            });
        }
        receiver
    }

    /// The subscription of one pair, as API/01 §32 describes it.
    pub fn subscription(&self, id: &str, project: &str, space: &str, entity_type: &str) -> Value {
        let expires = time::OffsetDateTime::now_utc() + LIFETIME;
        json!({
            "id": id,
            "type": "Subscription",
            "description": "Portal live updates of open data views (API/01 §32)",
            "entities": [{ "type": entity_type }],
            "notification": {
                "format": "keyValues",
                "endpoint": {
                    "uri": format!("{}/live-notify/{}", self.public_base, self.key(project, space, entity_type)),
                    "accept": "application/json"
                }
            },
            "expiresAt": expires
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
        })
    }

    async fn write(
        &self,
        project: &str,
        segment: &str,
        space: &str,
        entity_type: &str,
    ) -> Result<(), String> {
        let Some(writer) = &self.writer else {
            return Err("no gateway, realm client or organization domain to write with".into());
        };
        // One id per pair, so a second write updates; project and type are hashed into a name
        // segment, so a same-named space of another project writes a subscription of its own.
        let digest = Sha256::digest(format!("{project}\0{entity_type}").as_bytes());
        let id = writer.urn(segment, &format!("portal-live-{}", &hex(&digest)[..16]));
        let token = writer.token().await?;
        writer
            .write(
                &token,
                segment,
                &id,
                &self.subscription(&id, project, space, entity_type),
            )
            .await
    }

    /// Passes one notification on as "this type changed"; `false` for a key this process never
    /// made. The body is not read: nothing in it reaches a window (T-4876).
    pub fn deliver(&self, key: &str) -> bool {
        let Some(pair) = self.pair_of(key) else {
            return false;
        };
        let changed = Arc::new(Changed {
            entity_type: pair.2.clone(),
        });
        if let Some(sender) = self
            .channels
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&pair)
        {
            // No window left is not an error: the subscription expires on its own.
            let _ = sender.send(changed);
        }
        true
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The 32 bytes of a lowercase hex key; `None` for anything else.
fn unhex(key: &str) -> Option<Vec<u8>> {
    if key.len() != 64 {
        return None;
    }
    (0..64)
        .step_by(2)
        .map(|i| u8::from_str_radix(key.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_names_one_pair_and_only_this_process_makes_it() {
        let hub = LiveHub::new("https://portal.example/", None);
        let other = LiveHub::new("https://portal.example", None);
        let key = |hub: &LiveHub, project, space, t| hub.key(project, space, t);
        assert_eq!(
            key(&hub, "p", "bikes", "Station"),
            key(&hub, "p", "bikes", "Station")
        );
        assert_ne!(
            key(&hub, "p", "bikes", "Station"),
            key(&hub, "p", "bikes", "Road")
        );
        assert_ne!(
            key(&hub, "p", "bikes", "Station"),
            key(&hub, "p", "bikesS", "tation")
        );
        assert_ne!(
            key(&hub, "p", "bikes", "Station"),
            key(&hub, "q", "bikes", "Station")
        );
        assert_ne!(
            key(&hub, "p", "bikes", "Station"),
            key(&hub, "pbikes", "", "Station")
        );
        assert_ne!(
            key(&hub, "p", "bikes", "Station"),
            key(&other, "p", "bikes", "Station")
        );
        let body = hub.subscription("urn:ngsi-ld:Subscription:x", "p", "bikes", "Station");
        assert_eq!(
            body["notification"]["endpoint"]["uri"],
            format!(
                "https://portal.example/live-notify/{}",
                hub.key("p", "bikes", "Station")
            )
        );
        assert_eq!(body["entities"], json!([{ "type": "Station" }]));
    }

    #[tokio::test]
    async fn the_first_watcher_writes_the_subscription_once_through_the_space_surface() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/realms/jc/protocol/openid-connect/token"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "access_token": "t", "expires_in": 60 })),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/cs/helsinki-bikes/ngsi-ld/v1/subscriptions"))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;
        let writer = SubscriptionSync::new(
            server.uri(),
            &format!("{}/realms/jc", server.uri()),
            "portal".into(),
            crate::config::ClientAuth::Secret("secret".into()),
            "hel.fi",
        );
        let hub = Arc::new(LiveHub::new(
            "https://portal.example",
            Some(Arc::new(writer)),
        ));
        // F-POR4-012: the tenant is the space's URN segment, never its bare name.
        let _a = hub.watch("p", "bikes", "helsinki-bikes", "Station");
        let _b = hub.watch("p", "bikes", "helsinki-bikes", "Station");
        for _ in 0..50 {
            if !server
                .received_requests()
                .await
                .unwrap_or_default()
                .iter()
                .any(|r| r.url.path().contains("/subscriptions"))
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
        let written: Vec<Value> = server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|r| r.url.path().contains("/subscriptions"))
            .map(|r| serde_json::from_slice(&r.body).unwrap_or_default())
            .collect();
        assert_eq!(written.len(), 1, "one write for two windows");
        assert!(written[0]["id"]
            .as_str()
            .unwrap_or_default()
            .starts_with("urn:ngsi-ld:Subscription:hel.fi:helsinki-bikes:portal-live-"));
        assert_eq!(
            written[0]["notification"]["endpoint"]["uri"],
            format!(
                "https://portal.example/live-notify/{}",
                hub.key("p", "bikes", "Station")
            )
        );
    }

    /// F-POR4-021: a pair no window listens to any more is forgotten at the next watch.
    #[tokio::test]
    async fn a_pair_without_windows_is_forgotten() {
        let hub = Arc::new(LiveHub::new("https://portal.example", None));
        for space in ["a", "b", "c"] {
            drop(hub.watch("p", space, space, "Station"));
        }
        let _kept = hub.watch("p", "d", "d", "Station");
        assert_eq!(
            hub.keys.lock().map(|keys| keys.len()).unwrap_or_default(),
            1
        );
        assert_eq!(hub.channels.lock().map(|c| c.len()).unwrap_or_default(), 1);
        // F-R2POR4-015: its renew time goes with it.
        let written = hub.written.lock().map(|w| w.clone()).unwrap_or_default();
        assert!(
            written.keys().all(|(_, space, _)| space == "d"),
            "{written:?}"
        );
        assert!(!hub.deliver(&hub.key("p", "a", "Station")));
        assert!(hub.deliver(&hub.key("p", "d", "Station")));
    }

    #[tokio::test]
    async fn a_watcher_hears_only_that_its_pair_changed_and_an_unknown_key_is_refused() {
        let hub = Arc::new(LiveHub::new("https://portal.example", None));
        let mut stations = hub.watch("p", "bikes", "p-bikes", "Station");
        let mut roads = hub.watch("p", "bikes", "p-bikes", "Road");
        let mut elsewhere = hub.watch("q", "bikes", "q-bikes", "Station");
        assert!(hub.deliver(&hub.key("p", "bikes", "Station")));
        let changed = stations.recv().await.expect("a change");
        assert_eq!(
            serde_json::to_value(changed.as_ref()).expect("json"),
            json!({ "type": "Station" }),
            "no id, attribute name or value reaches a window"
        );
        assert!(roads.try_recv().is_err());
        assert!(
            elsewhere.try_recv().is_err(),
            "another project's same-named space"
        );
        assert!(!hub.deliver("0".repeat(64).as_str()));
        assert!(!hub.deliver("not hex"));
        assert!(!hub.deliver(&hub.key("p", "bikes", "Station")[..62]));
        assert!(!hub.deliver(&LiveHub::new("x", None).key("p", "bikes", "Station")));
        // A key made for a pair nobody watches is unknown too.
        assert!(!hub.deliver(&hub.key("p", "bikes", "Tram")));
    }

    #[test]
    fn a_key_reads_back_as_its_bytes_and_nothing_else_does() {
        assert_eq!(unhex(&"0a".repeat(32)), Some(vec![10; 32]));
        assert_eq!(unhex(&"0a".repeat(31)), None);
        assert_eq!(unhex(&format!("{}zz", "0a".repeat(31))), None);
        assert_eq!(unhex(&format!("é{}", "0".repeat(62))), None);
    }
}
