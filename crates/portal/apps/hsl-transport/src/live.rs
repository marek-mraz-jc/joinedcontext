//! The fleet from the Endpoint's WebSocket (API/07, EP-106, T-3581).
//!
//! One socket for the whole process, however many browsers watch: it opens
//! `{endpoint}ws` anonymously (the Endpoint is public, AP-28), subscribes to `Vehicle` on
//! `location`, and each notification moves the buses it names. The socket closes every five
//! minutes by design (`x-closing: lifetime`); the next one resumes from the last `x-seq`, so
//! nothing inside the 30-second window is lost. Whenever the stream cannot be trusted to be
//! complete (a fresh subscription, `x-gap`, a failed resume) the fleet is read once over REST.
//! While no socket is up, the map says since when the feed is lost.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use crate::{simple_value, App, Vehicle, FLEET, TYPE};

/// The subprotocol of the Endpoint's socket (API/07 §1).
const PROTOCOL: &str = "ngsi-ld-ws";
/// The attributes a notification carries: what the map draws.
const ATTRIBUTES: [&str; 5] = ["location", "heading", "speed", "route", "transportMode"];
/// The longest wait between two connection attempts.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Where the last socket left off, so the next one resumes instead of starting over.
#[derive(Clone, Default)]
struct Resume {
    connection: String,
    token: String,
    seq: u64,
}

/// The resume secret stays out of every log line.
impl std::fmt::Debug for Resume {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resume")
            .field("connection", &self.connection)
            .field("token", &"[redacted]")
            .field("seq", &self.seq)
            .finish()
    }
}

/// A plain or TLS stream under the socket.
trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// Opens the socket: TCP, then for `wss` rustls with the webpki-roots 1.x public roots (the same
/// roots as the REST client), then the WebSocket upgrade.
async fn open(request: Request) -> Result<WebSocketStream<Box<dyn Io>>, String> {
    let uri = request.uri();
    let host = uri
        .host()
        .ok_or("the socket URL has no host")?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_owned();
    let tls = match uri.scheme_str() {
        Some("wss") => true,
        Some("ws") => false,
        _ => return Err(format!("the socket URL {uri} is not ws:// or wss://")),
    };
    let port = uri.port_u16().unwrap_or(if tls { 443 } else { 80 });
    let tcp = TcpStream::connect((host.as_str(), port))
        .await
        .map_err(|e| format!("connecting to {host}:{port}: {e}"))?;
    let stream: Box<dyn Io> = if tls {
        let name = ServerName::try_from(host).map_err(|e| e.to_string())?;
        let roots = RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        let config = ClientConfig::builder_with_provider(Arc::new(
            tokio_rustls::rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
        let tls = TlsConnector::from(Arc::new(config))
            .connect(name, tcp)
            .await
            .map_err(|e| e.to_string())?;
        Box::new(tls)
    } else {
        Box::new(tcp)
    };
    let (socket, _) = tokio_tungstenite::client_async(request, stream)
        .await
        .map_err(|e| e.to_string())?;
    Ok(socket)
}

/// `{endpoint}ws`, with the scheme a WebSocket client speaks.
pub fn socket_url(endpoint_url: &str) -> String {
    let url = format!("{endpoint_url}ws");
    if let Some(rest) = url.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = url.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        url
    }
}

/// The subscription the socket asks for: every `Vehicle`, told on a move.
pub fn subscription() -> Value {
    json!({
        "metadata": { "x-op": "x-subscribe", "x-id": "fleet" },
        "body": {
            "type": "Subscription",
            "entities": [{ "type": TYPE }],
            "watchedAttributes": ["location"],
            "notification": { "attributes": ATTRIBUTES },
        },
    })
}

/// One bus from a notification's normalized entity, merged over what the map already holds: an
/// attribute the notification leaves out keeps its value, and a bus the map has never placed
/// needs a point. `None` for anything else.
pub fn vehicle_of(entity: &Value, known: Option<&Vehicle>) -> Option<Vehicle> {
    let id = entity.get("id")?.as_str()?.to_owned();
    let point = entity
        .get("location")
        .and_then(simple_value)
        .and_then(|geometry| {
            if geometry.get("type")?.as_str()? != "Point" {
                return None;
            }
            let coordinates = geometry.get("coordinates")?.as_array()?;
            Some([
                coordinates.first()?.as_f64()?,
                coordinates.get(1)?.as_f64()?,
            ])
        });
    let coordinates = point.or_else(|| known.map(|vehicle| vehicle.coordinates))?;
    let number = |name: &str| entity.get(name).and_then(simple_value)?.as_f64();
    let text = |name: &str| {
        entity
            .get(name)
            .and_then(simple_value)?
            .as_str()
            .map(str::to_owned)
    };
    Some(Vehicle {
        id,
        coordinates,
        bearing: number("heading").or_else(|| known?.bearing),
        speed: number("speed").or_else(|| known?.speed),
        ref_line: text("route").or_else(|| known?.ref_line.clone()),
        mode: text("transportMode").or_else(|| known?.mode.clone()),
    })
}

/// The time the bus was at its point, from `location.observedAt`, for the latency (T-3581).
fn observed_at(entity: &Value) -> Option<SystemTime> {
    let at = entity.get("location")?.get("observedAt")?.as_str()?;
    let parsed = chrono::DateTime::parse_from_rfc3339(at).ok()?;
    let millis = u64::try_from(parsed.timestamp_millis()).ok()?;
    Some(SystemTime::UNIX_EPOCH + Duration::from_millis(millis))
}

impl App {
    /// Moves the buses of one notification and tells the browsers what moved.
    pub fn notified(&self, body: &Value) -> Vec<Vehicle> {
        let entities = body
            .get("data")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let now = SystemTime::now();
        let mut changed = Vec::new();
        if let Ok(mut fleet) = self.fleet.write() {
            for entity in entities {
                if let Some(at) = observed_at(entity) {
                    if let Ok(late) = now.duration_since(at) {
                        self.metrics.latency(late);
                    }
                }
                let id = entity.get("id").and_then(Value::as_str).unwrap_or_default();
                let known = fleet.get(id);
                if known.is_none() && fleet.len() >= FLEET {
                    continue;
                }
                if let Some(vehicle) = vehicle_of(entity, known) {
                    if known != Some(&vehicle) {
                        fleet.insert(vehicle.id.clone(), vehicle.clone());
                        changed.push(vehicle);
                    }
                }
            }
        }
        if !changed.is_empty() {
            let _ = self.changes.send(changed.clone());
        }
        changed
    }

    /// The socket, for the process: connect, subscribe or resume, follow, and again.
    pub async fn follow(self: Arc<Self>) {
        let url = socket_url(&self.config.endpoint_url);
        let mut resume: Option<Resume> = None;
        let mut backoff = Duration::from_secs(1);
        loop {
            let opened = Instant::now();
            match self.session(&url, &mut resume).await {
                Ok(reason) => tracing::info!(%reason, "the socket closed"),
                Err(error) => tracing::warn!(%error, "the socket failed"),
            }
            self.metrics.reconnect();
            // A lifetime close is the protocol at work; resuming at once keeps the map live.
            if resume.is_some() && opened.elapsed() > MAX_BACKOFF {
                backoff = Duration::from_secs(1);
                continue;
            }
            self.feed_lost();
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    }

    /// One socket from upgrade to close; `Ok` names why the server closed it.
    async fn session(&self, url: &str, resume: &mut Option<Resume>) -> Result<String, String> {
        let mut request = url.into_client_request().map_err(|e| e.to_string())?;
        request
            .headers_mut()
            .insert("sec-websocket-protocol", HeaderValue::from_static(PROTOCOL));
        let mut socket = open(request).await?;
        let mut welcome: Option<Resume> = None;
        while let Some(frame) = socket.next().await {
            let text = match frame.map_err(|e| e.to_string())? {
                Message::Text(text) => text,
                Message::Close(_) => return Ok("closed".to_owned()),
                _ => continue,
            };
            let Ok(message) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            let metadata = &message["metadata"];
            let field = |name: &str| metadata[name].as_str().unwrap_or_default().to_owned();
            match metadata["x-op"].as_str().unwrap_or_default() {
                "x-welcome" => {
                    welcome = Some(Resume {
                        connection: field("x-connection"),
                        token: field("x-resume-token"),
                        seq: 0,
                    });
                    let first = match resume.take() {
                        Some(previous) => json!({ "metadata": {
                            "x-op": "x-resume", "x-id": "resume",
                            "x-connection": previous.connection,
                            "x-resume-token": previous.token,
                            "x-seq": previous.seq,
                        } }),
                        None => subscription(),
                    };
                    socket
                        .send(Message::text(first.to_string()))
                        .await
                        .map_err(|e| e.to_string())?;
                }
                "x-result" => {
                    let status = metadata["x-status"].as_u64().unwrap_or_default();
                    match (metadata["x-id"].as_str(), status) {
                        (Some("resume"), 200..=299) => self.feed_live(),
                        // The window passed: start over, and read what the gap hid.
                        (Some("resume"), _) => socket
                            .send(Message::text(subscription().to_string()))
                            .await
                            .map_err(|e| e.to_string())?,
                        (Some("fleet"), 200..=299) => self.reread().await,
                        (_, status) => return Err(format!("the subscription answered {status}")),
                    }
                }
                "x-notification" => {
                    if let Some(current) = welcome.as_mut() {
                        current.seq = metadata["x-seq"].as_u64().unwrap_or(current.seq);
                    }
                    self.metrics.notification();
                    self.notified(&message["body"]);
                }
                "x-gap" => {
                    self.metrics.gap();
                    self.feed_lost();
                    self.reread().await;
                }
                "x-closing" => {
                    *resume = welcome.take();
                    return Ok(field("x-reason"));
                }
                _ => {}
            }
            // Every frame moves the resume point, so a close without `x-closing` resumes too.
            resume.clone_from(&welcome);
        }
        Ok("ended".to_owned())
    }

    /// The whole fleet over REST, after which the map is complete again.
    async fn reread(&self) {
        match self.reload().await {
            Ok(_) => self.feed_live(),
            Err(error) => tracing::warn!(%error, "the fleet was not read again"),
        }
    }

    fn feed_live(&self) {
        self.feed.send_if_modified(|lost| lost.take().is_some());
    }

    fn feed_lost(&self) {
        self.feed.send_if_modified(|lost| {
            if lost.is_some() {
                return false;
            }
            *lost = Some(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
            true
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_socket_url_without_a_websocket_scheme_is_refused() {
        let request = "http://127.0.0.1:9/ws"
            .into_client_request()
            .expect("a request");
        let error = open(request).await.err().expect("refused");
        assert!(error.contains("not ws:// or wss://"), "{error}");
    }

    #[test]
    fn a_resume_debug_never_shows_the_token() {
        let resume = Resume {
            token: "s3cr3t-test".into(),
            ..Resume::default()
        };
        let text = format!("{resume:?}");
        assert!(
            !text.contains("s3cr3t-test") && text.contains("[redacted]"),
            "{text}"
        );
    }

    #[test]
    fn the_socket_url_follows_the_endpoints_scheme() {
        assert_eq!(
            socket_url("https://h/api/endpoint/s/"),
            "wss://h/api/endpoint/s/ws"
        );
        assert_eq!(
            socket_url("http://127.0.0.1:9/api/endpoint/s/"),
            "ws://127.0.0.1:9/api/endpoint/s/ws"
        );
    }

    #[test]
    fn a_partial_notification_keeps_what_it_leaves_out_and_an_unplaced_bus_waits() {
        let known = Vehicle {
            id: "urn:ngsi-ld:Vehicle:1".into(),
            coordinates: [24.9, 60.1],
            bearing: Some(90.0),
            speed: Some(5.0),
            ref_line: Some("550".into()),
            mode: Some("bus".into()),
        };
        let moved = vehicle_of(
            &json!({ "id": "urn:ngsi-ld:Vehicle:1", "location": { "type": "GeoProperty", "value": { "type": "Point", "coordinates": [25.0, 60.2] } } }),
            Some(&known),
        )
        .expect("a bus");
        assert_eq!(moved.coordinates, [25.0, 60.2]);
        assert_eq!(
            (moved.bearing, moved.ref_line.as_deref()),
            (Some(90.0), Some("550"))
        );
        assert_eq!(
            vehicle_of(&json!({ "id": "urn:ngsi-ld:Vehicle:2", "speed": 3 }), None),
            None
        );
    }
}
