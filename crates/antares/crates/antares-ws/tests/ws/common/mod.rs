// SPDX-License-Identifier: EUPL-1.2
//! One broker on a real socket with both WebSocket surfaces and the sink,
//! a client that speaks the envelope, and the HTTP calls the tests make
//! beside it.

pub mod asyncapi;

use antares_ops::test_kit::TestBroker;
use antares_ws::{Hub, Limits, Sink, StaMqtt, ViewWs, Ws};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue, Response};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// How long a test waits for a message it expects.
pub const WAIT: Duration = Duration::from_secs(10);

/// How long [`until`] waits for the hub to reach a state: the notifier
/// runs beside the test, so under CI load it gets far more than [`WAIT`].
pub const SETTLE: Duration = Duration::from_secs(60);

/// Tests take a shared turn; one that pushes megabytes through a debug
/// broker takes it alone, so the others' waits do not depend on its load.
static TURN: tokio::sync::RwLock<()> = tokio::sync::RwLock::const_new(());

/// A test's turn on the host: one of the guards, held until it ends.
pub struct Turn {
    _shared: Option<tokio::sync::RwLockReadGuard<'static, ()>>,
    _alone: Option<tokio::sync::RwLockWriteGuard<'static, ()>>,
}

/// A broker serving both surfaces on `addr`.
pub struct Broker {
    /// Held for the test's run.
    pub _turn: Turn,
    /// Kept alive: its reload loop serves `/q/views`.
    pub _kept: TestBroker,
    /// The hub the surfaces and the sink share.
    pub hub: Arc<Hub>,
    /// Where it listens.
    pub addr: SocketAddr,
    /// An HTTP client for the NGSI-LD API beside the sockets.
    pub http: Http,
}

/// The subject header the tests' policy engine reads.
pub const USER: &str = "x-user";

/// Start a broker under `limits`.
pub async fn broker(limits: Limits) -> Result<Broker, String> {
    broker_asking(limits, None).await
}

/// Start a broker under `limits` once no other test runs.
pub async fn broker_alone(limits: Limits) -> Result<Broker, String> {
    let turn = Turn {
        _shared: None,
        _alone: Some(TURN.write().await),
    };
    start(limits, None, turn).await
}

/// Start a broker under `limits` asking `engine`.
pub async fn broker_asking(
    limits: Limits,
    engine: Option<Arc<dyn antares_ops::policy::PolicyEngine>>,
) -> Result<Broker, String> {
    let turn = Turn {
        _shared: Some(TURN.read().await),
        _alone: None,
    };
    start(limits, engine, turn).await
}

async fn start(
    limits: Limits,
    engine: Option<Arc<dyn antares_ops::policy::PolicyEngine>>,
    turn: Turn,
) -> Result<Broker, String> {
    // read once by the broker at first use, before any broker of this
    // binary starts
    static SUBJECT: std::sync::Once = std::sync::Once::new();
    SUBJECT.call_once(|| std::env::set_var("ANTARES_POLICY_SUBJECT_HEADERS", USER));
    let hub = Hub::new(limits);
    let broker = TestBroker::with_sinks(vec![Box::new(Sink::new(hub.clone()))], engine).await?;
    let app = broker.serve_mounted(vec![
        antares_ops::mount(Ws::new(hub.clone())),
        antares_ops::mount(ViewWs::new(hub.clone())),
        antares_ops::mount(StaMqtt::new(hub.clone())),
        antares_ops::mount(StubSta),
    ])?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| e.to_string())?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    tokio::spawn(async move {
        let _served = axum::serve(listener, app).await;
    });
    Ok(Broker {
        _turn: turn,
        _kept: broker,
        hub,
        addr,
        http: Http { addr },
    })
}

/// The HTTP side: one request per call, over its own connection.
#[derive(Clone, Copy)]
pub struct Http {
    addr: SocketAddr,
}

impl Http {
    /// `method path` with `headers` and a JSON `body`; the status and the
    /// body as JSON (`Null` when empty).
    pub async fn call(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<&Value>,
    ) -> Result<(u16, Value), String> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let payload = body.map(Value::to_string).unwrap_or_default();
        let mut req =
            format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
        for (k, v) in headers {
            let _w = write!(req, "{k}: {v}\r\n");
        }
        if body.is_some() {
            req.push_str("Content-Type: application/json\r\n");
        }
        let _w = write!(req, "Content-Length: {}\r\n\r\n{payload}", payload.len());
        let mut s = TcpStream::connect(self.addr)
            .await
            .map_err(|e| e.to_string())?;
        s.write_all(req.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        let mut raw = Vec::new();
        s.read_to_end(&mut raw).await.map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&raw);
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| format!("no status in {head:?}"))?;
        let chunked = head
            .to_ascii_lowercase()
            .contains("transfer-encoding: chunked");
        let body = if chunked {
            dechunk(body)?
        } else {
            body.to_owned()
        };
        let json = if body.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(body.trim()).map_err(|e| format!("{e}: {body}"))?
        };
        Ok((status, json))
    }

    /// Create a `Car` in `tenant` (`None`: the default Tenant).
    pub async fn car(&self, tenant: Option<&str>, id: &str, speed: i64) -> Result<(), String> {
        let headers: Vec<(&str, &str)> = tenant.map(|t| ("NGSILD-Tenant", t)).into_iter().collect();
        let body = json!({"id": id, "type": "Car", "speed": {"type": "Property", "value": speed}});
        let (status, answer) = self
            .call("POST", "/ngsi-ld/v1/entities", &headers, Some(&body))
            .await?;
        (status == 201)
            .then_some(())
            .ok_or(format!("create {id}: {status} {answer}"))
    }

    /// Set `speed` of a Car.
    pub async fn speed(&self, tenant: Option<&str>, id: &str, speed: i64) -> Result<(), String> {
        let headers: Vec<(&str, &str)> = tenant.map(|t| ("NGSILD-Tenant", t)).into_iter().collect();
        let body = json!({"speed": {"type": "Property", "value": speed}});
        let path = format!("/ngsi-ld/v1/entities/{id}/attrs");
        let (status, answer) = self.call("PATCH", &path, &headers, Some(&body)).await?;
        (status == 204)
            .then_some(())
            .ok_or(format!("speed {id}: {status} {answer}"))
    }
}

/// The payload of a chunked HTTP/1.1 body.
fn dechunk(mut rest: &str) -> Result<String, String> {
    let mut out = String::new();
    loop {
        let (size, tail) = rest
            .split_once("\r\n")
            .ok_or("a chunk without a size line")?;
        let size = usize::from_str_radix(size.trim(), 16)
            .map_err(|e| format!("chunk size {size:?}: {e}"))?;
        if size == 0 {
            return Ok(out);
        }
        out.push_str(tail.get(..size).ok_or("a chunk shorter than its size")?);
        rest = tail
            .get(size..)
            .and_then(|t| t.strip_prefix("\r\n"))
            .ok_or("a chunk without its CRLF")?;
    }
}

/// One client socket.
pub struct Client {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    /// `x-connection` of its welcome.
    pub connection: String,
    /// `x-resume-token` of its welcome.
    pub token: String,
}

/// The upgrade's answer when it was refused.
pub type Refusal = Response<Option<Vec<u8>>>;

/// Open `path` with `headers`; the refusal when the upgrade is not
/// answered with 101.
pub async fn open(
    addr: SocketAddr,
    path: &str,
    headers: &[(&str, &str)],
) -> Result<Result<Client, Refusal>, String> {
    let mut req = format!("ws://{addr}{path}")
        .into_client_request()
        .map_err(|e| e.to_string())?;
    for (k, v) in headers {
        req.headers_mut().append(
            HeaderName::from_bytes(k.as_bytes()).map_err(|e| e.to_string())?,
            HeaderValue::from_str(v).map_err(|e| e.to_string())?,
        );
    }
    let ws = match tokio_tungstenite::connect_async(req).await {
        Ok((ws, _)) => ws,
        Err(tokio_tungstenite::tungstenite::Error::Http(refused)) => return Ok(Err(*refused)),
        Err(e) => return Err(e.to_string()),
    };
    welcomed(ws).await.map(Ok)
}

/// Open the root with a receive buffer of `bytes` fixed before the
/// connect, which turns the kernel's autotuning off: a client that stops
/// reading then stalls the server's writes after about that much, whatever
/// the host's `tcp_rmem` ceiling.
pub async fn connect_small(addr: SocketAddr, bytes: u32) -> Result<Client, String> {
    let socket = tokio::net::TcpSocket::new_v4().map_err(|e| e.to_string())?;
    socket
        .set_recv_buffer_size(bytes)
        .map_err(|e| e.to_string())?;
    let stream = socket.connect(addr).await.map_err(|e| e.to_string())?;
    let req = format!("ws://{addr}/x/ws/ngsi-ld/v1")
        .into_client_request()
        .map_err(|e| e.to_string())?;
    let (ws, _) = tokio_tungstenite::client_async(req, MaybeTlsStream::Plain(stream))
        .await
        .map_err(|e| e.to_string())?;
    welcomed(ws).await
}

/// A client on `ws` once its welcome is read.
async fn welcomed(ws: WebSocketStream<MaybeTlsStream<TcpStream>>) -> Result<Client, String> {
    let mut client = Client {
        ws,
        connection: String::new(),
        token: String::new(),
    };
    let welcome = client.next().await?;
    if welcome.pointer("/metadata/x-op") != Some(&json!("x-welcome")) {
        return Err(format!("not a welcome: {welcome}"));
    }
    client.connection = text(&welcome, "/metadata/x-connection");
    client.token = text(&welcome, "/metadata/x-resume-token");
    Ok(client)
}

/// Open `path`, which must be admitted.
pub async fn connect(
    addr: SocketAddr,
    path: &str,
    headers: &[(&str, &str)],
) -> Result<Client, String> {
    open(addr, path, headers)
        .await?
        .map_err(|r| format!("upgrade refused: {}", r.status()))
}

/// A server message as JSON, once `asyncapi-ws.yaml` describes it.
fn described(text: &str) -> Result<Value, String> {
    let m: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    asyncapi::check_server(&m)?;
    Ok(m)
}

/// The string at `pointer`, empty when absent.
#[must_use]
pub fn text(v: &Value, pointer: &str) -> String {
    v.pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

impl Client {
    /// Send one text frame.
    pub async fn send_text(&mut self, text: String) -> Result<(), String> {
        self.ws
            .send(Message::Text(text.into()))
            .await
            .map_err(|e| e.to_string())
    }

    /// Send one envelope.
    pub async fn send(&mut self, metadata: Value, body: Option<Value>) -> Result<(), String> {
        let mut m = json!({"metadata": metadata});
        if let (Some(b), Some(o)) = (body, m.as_object_mut()) {
            o.insert("body".into(), b);
        }
        self.send_text(m.to_string()).await
    }

    /// The next text message as JSON, within [`WAIT`].
    pub async fn next(&mut self) -> Result<Value, String> {
        loop {
            let frame = tokio::time::timeout(WAIT, self.ws.next())
                .await
                .map_err(|_| "no message within the wait".to_owned())?
                .ok_or("the socket closed")?
                .map_err(|e| e.to_string())?;
            match frame {
                Message::Text(t) => return described(t.as_str()),
                Message::Close(c) => return Err(format!("closed: {c:?}")),
                _ => {}
            }
        }
    }

    /// The close frame's code once the server closes, skipping messages.
    pub async fn closed(&mut self) -> Result<(u16, Vec<Value>), String> {
        let mut seen = Vec::new();
        loop {
            let frame = tokio::time::timeout(WAIT, self.ws.next())
                .await
                .map_err(|_| "not closed within the wait".to_owned())?;
            match frame {
                Some(Ok(Message::Text(t))) => seen.push(described(t.as_str())?),
                Some(Ok(Message::Close(Some(c)))) => {
                    // answer it, as a browser does: the next read sends
                    // the close frame tungstenite queued
                    let _answered = tokio::time::timeout(WAIT, self.ws.next()).await;
                    return Ok((c.code.into(), seen));
                }
                Some(Ok(Message::Close(None))) | None => return Ok((1005, seen)),
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(format!("{e}; seen {seen:?}")),
            }
        }
    }

    /// `x-subscribe` with `body`; the `x-result`.
    pub async fn subscribe(&mut self, body: Value) -> Result<Value, String> {
        self.send(json!({"x-op": "x-subscribe", "x-id": "s"}), Some(body))
            .await?;
        self.result().await
    }

    /// The next `x-result`.
    pub async fn result(&mut self) -> Result<Value, String> {
        let m = self.next().await?;
        if m.pointer("/metadata/x-op") == Some(&json!("x-result")) {
            Ok(m)
        } else {
            Err(format!("not a result: {m}"))
        }
    }

    /// The next `x-notification`.
    pub async fn notification(&mut self) -> Result<Value, String> {
        let m = self.next().await?;
        if m.pointer("/metadata/x-op") == Some(&json!("x-notification")) {
            Ok(m)
        } else {
            Err(format!("not a notification: {m}"))
        }
    }

    /// The raw stream, for frames the envelope helpers do not send.
    pub fn ws_mut(&mut self) -> &mut WebSocketStream<MaybeTlsStream<TcpStream>> {
        &mut self.ws
    }

    /// Close the socket from the client side.
    pub async fn close(mut self) {
        let _closed = self.ws.close(None).await;
    }
}

/// A notification of `subscription` as the notifier hands it to a sink.
pub fn outbound(subscription: Option<&str>, tenant: Option<&str>) -> antares_ops::Outbound {
    let mut body = json!({
        "id": "urn:ngsi-ld:Notification:1", "type": "Notification",
        "notifiedAt": "2026-10-10T00:00:00Z",
        "data": [{"id": "urn:ngsi-ld:Car:r", "type": "Car", "speed": {"type": "Property", "value": 7}}]
    });
    if let (Some(s), Some(o)) = (subscription, body.as_object_mut()) {
        o.insert("subscriptionId".into(), s.into());
    }
    antares_ops::Outbound {
        body,
        accept: "application/json".into(),
        link: String::new(),
        receiver_info: tenant
            .map(|t| ("NGSILD-Tenant".to_owned(), t.to_owned()))
            .into_iter()
            .collect(),
        notifier_info: Vec::new(),
    }
}

/// The TS 104 243 message a pod renders for `subscription`, as another pod
/// forwards it: a Notification whose id ends in `n`.
#[must_use]
pub fn rendered(subscription: &str, n: i64) -> Value {
    json!({
        "metadata": {"Content-Type": "application/ld+json"},
        "body": {
            "id": format!("urn:ngsi-ld:Notification:{n}"), "type": "Notification",
            "subscriptionId": subscription, "notifiedAt": "2026-10-10T00:00:00Z", "data": []
        }
    })
}

/// A Subscription to Cars' `speed`.
#[must_use]
pub fn cars() -> Value {
    json!({"type": "Subscription", "entities": [{"type": "Car"}], "watchedAttributes": ["speed"], "notification": {"attributes": ["speed"]}})
}

/// The status a result carries.
#[must_use]
pub fn status(result: &Value) -> u64 {
    result
        .pointer("/metadata/x-status")
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

/// `v` with `x` at `path`, making the objects on the way.
#[must_use]
pub fn with(mut v: Value, path: &[&str], x: Value) -> Value {
    let Some((last, parents)) = path.split_last() else {
        return x;
    };
    let mut cur = &mut v;
    for k in parents {
        let Some(o) = cur.as_object_mut() else {
            return v;
        };
        cur = o.entry(*k).or_insert_with(|| json!({}));
    }
    if let Some(o) = cur.as_object_mut() {
        o.insert((*last).to_owned(), x);
    }
    v
}

/// Wait until `what` of the hub holds, within [`SETTLE`].
pub async fn until(hub: &Hub, what: impl Fn(&Hub) -> bool) -> Result<(), String> {
    let waited = tokio::time::timeout(SETTLE, async {
        while !what(hub) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    waited.map_err(|_| {
        format!(
            "not within the wait: {} connections, {} attached, {} overflows, {} queued",
            hub.connections(),
            hub.attached(),
            hub.overflows(),
            hub.queued()
        )
    })
}

impl Client {
    /// `x-resume` of `connection` with `token` after `seq`; the `x-result`.
    pub async fn resume(
        &mut self,
        connection: &str,
        token: &str,
        seq: u64,
    ) -> Result<Value, String> {
        let m = json!({"x-op": "x-resume", "x-id": "r", "x-connection": connection, "x-resume-token": token, "x-seq": seq});
        self.send(m, None).await?;
        self.result().await
    }
}

/// The `SensorThings` answers [`StubSta`] gives, by `{view}/{path}` and `?{query}` when there is
/// one; the crate may not depend on the `SensorThings` surface (module rule surface-dev), so a
/// test scripts what each read answers.
static STA: std::sync::Mutex<std::collections::BTreeMap<String, (u16, String)>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// Script the answer of `GET /x/views/{key}` below `/sta/v1.1/`.
pub fn sta(key: &str, status: u16, body: &Value) {
    STA.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(key.to_owned(), (status, body.to_string()));
}

/// A stand-in for the `SensorThings` surface: the scripted answer, else 404.
pub struct StubSta;

fn scripted(view: &str, rest: &str, query: Option<String>) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    let key = match query {
        Some(q) => format!("{view}/{rest}?{q}"),
        None => format!("{view}/{rest}"),
    };
    let answer = STA
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
        .cloned();
    let (status, body) = answer.unwrap_or((404, "{}".to_owned()));
    let status = axum::http::StatusCode::from_u16(status).unwrap_or_default();
    (status, body).into_response()
}

impl antares_ops::Surface for StubSta {
    fn name(&self) -> &'static str {
        "sta"
    }
    fn prefix(&self) -> &'static str {
        "/x/views"
    }
    fn version_info(&self) -> Value {
        Value::Null
    }
    fn router(&self, _ops: antares_ops::Ops) -> axum::Router {
        use axum::extract::{Path, RawQuery};
        axum::Router::new()
            .route(
                "/{view}/sta/v1.1",
                axum::routing::get(|Path(view): Path<String>, RawQuery(q): RawQuery| {
                    std::future::ready(scripted(&view, "", q))
                }),
            )
            .route(
                "/{view}/sta/v1.1/{*rest}",
                axum::routing::get(
                    |Path((view, rest)): Path<(String, String)>, RawQuery(q): RawQuery| {
                        std::future::ready(scripted(&view, &rest, q))
                    },
                ),
            )
    }
}
