//! E3: the `websocket` input, ported from Bento's `internal/impl/io/input_websocket_test.go`
//! (`bento_input_websocket_test_*`), against a small RFC 6455 server written here (handshake and
//! frames by hand) so the test does not depend on a websocket server library.
#![cfg(feature = "websocket")]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use ekiben_bloblang::Environment;
use ekiben_core::{
    BatchError, ComponentConfig, CoreError, Input, OrgId, OrgQuotas, Registry, ResourceManager,
    StreamContext, StreamMetrics,
};
use ekiben_io_http::websocket::{oauth1_header, WebsocketConf};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};

const OP_TEXT: u8 = 1;
const OP_BINARY: u8 = 2;
const OP_CLOSE: u8 = 8;

/// One accepted websocket connection, server side.
struct ServerConn {
    headers: HashMap<String, String>,
    stream: TcpStream,
}

impl ServerConn {
    async fn send(&mut self, opcode: u8, payload: &[u8]) {
        let mut frame = vec![0x80 | opcode];
        if payload.len() < 126 {
            frame.push(payload.len() as u8);
        } else {
            frame.push(126);
            frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        }
        frame.extend_from_slice(payload);
        self.stream.write_all(&frame).await.unwrap();
    }

    async fn recv(&mut self) -> (u8, Vec<u8>) {
        let mut head = [0u8; 2];
        self.stream.read_exact(&mut head).await.unwrap();
        let opcode = head[0] & 0x0f;
        let masked = head[1] & 0x80 != 0;
        let mut len = u64::from(head[1] & 0x7f);
        if len == 126 {
            let mut b = [0u8; 2];
            self.stream.read_exact(&mut b).await.unwrap();
            len = u64::from(u16::from_be_bytes(b));
        } else if len == 127 {
            let mut b = [0u8; 8];
            self.stream.read_exact(&mut b).await.unwrap();
            len = u64::from_be_bytes(b);
        }
        let mut mask = [0u8; 4];
        if masked {
            self.stream.read_exact(&mut mask).await.unwrap();
        }
        let mut payload = vec![0u8; len as usize];
        self.stream.read_exact(&mut payload).await.unwrap();
        if masked {
            for (i, b) in payload.iter_mut().enumerate() {
                *b ^= mask[i % 4];
            }
        }
        (opcode, payload)
    }
}

async fn handshake(mut stream: TcpStream) -> ServerConn {
    let mut raw = Vec::new();
    let mut byte = [0u8; 1];
    while !raw.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).await.unwrap();
        raw.push(byte[0]);
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let headers: HashMap<String, String> = text
        .lines()
        .skip(1)
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let key = headers
        .get("sec-websocket-key")
        .cloned()
        .unwrap_or_default();
    let accept = base64::engine::general_purpose::STANDARD.encode(Sha1::digest(
        format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11").as_bytes(),
    ));
    let resp = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    stream.write_all(resp.as_bytes()).await.unwrap();
    ServerConn { headers, stream }
}

/// A websocket server on loopback; every accepted connection arrives on the channel.
async fn ws_server() -> (String, mpsc::UnboundedReceiver<ServerConn>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let tx = tx.clone();
            tokio::spawn(async move {
                let _ = tx.send(handshake(stream).await);
            });
        }
    });
    (format!("ws://{addr}/ws"), rx)
}

async fn accepted(rx: &mut mpsc::UnboundedReceiver<ServerConn>) -> ServerConn {
    tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("the input connects")
        .expect("server running")
}

fn context(
    allow_private_ips: bool,
    env: Environment,
) -> (Arc<Registry>, StreamContext, watch::Sender<bool>) {
    let reg = Arc::new(Registry::new());
    ekiben_io_http::register_all(&reg);
    let resources = ResourceManager::new();
    let org_id = OrgId::new("org-ws");
    let org = resources.set_org_quotas(
        &org_id,
        OrgQuotas {
            allow_private_ips,
            ..Default::default()
        },
    );
    let (tx, rx) = watch::channel(false);
    let ctx = StreamContext {
        stream_id: "ws-test".to_string(),
        org,
        resources,
        registry: reg.clone(),
        blobl_env: env,
        metrics: StreamMetrics::new("ws-test", "org-ws"),
        label: String::new(),
        path: "root.input".to_string(),
        file_root: None,
        env_vars: Arc::new(HashMap::new()),
        shutdown_rx: rx,
    };
    (reg, ctx, tx)
}

fn build(conf: Value, ctx: &StreamContext, reg: &Registry) -> Box<dyn Input> {
    reg.build_input(
        &ComponentConfig {
            label: String::new(),
            component_type: "websocket".to_string(),
            config: conf,
            processors: Vec::new(),
        },
        ctx,
    )
    .expect("the input builds")
}

async fn read_payload(
    input: &mut Box<dyn Input>,
    ctx: &StreamContext,
) -> (Vec<u8>, ekiben_core::AckResponder) {
    let (batch, ack) = tokio::time::timeout(Duration::from_secs(10), input.read_batch(ctx))
        .await
        .expect("a message arrives")
        .expect("read");
    assert_eq!(batch.len(), 1);
    (batch[0].as_bytes_ref().to_vec(), ack)
}

#[tokio::test]
async fn bento_input_websocket_test_websocket_basic() {
    let (url, mut conns) = ws_server().await;
    let (reg, ctx, _stop) = context(true, Environment::new());
    let mut input = build(json!({"url": url}), &ctx, &reg);
    input.connect(&ctx).await.expect("connects");
    let mut server = accepted(&mut conns).await;
    let expected = ["foo", "bar", "baz"];
    for (i, msg) in expected.iter().enumerate() {
        let op = if i % 2 == 0 { OP_TEXT } else { OP_BINARY };
        server.send(op, msg.as_bytes()).await;
    }
    for msg in expected {
        let (payload, ack) = read_payload(&mut input, &ctx).await;
        assert_eq!(payload, msg.as_bytes());
        ack.ack();
    }
    input.close().await.unwrap();
}

#[tokio::test]
async fn bento_input_websocket_test_websocket_open_msg() {
    struct Case {
        conf: Value,
        want: Vec<(u8, &'static str)>,
    }
    let cases = [
        Case {
            conf: json!({"open_message": "bar", "open_message_type": "binary"}),
            want: vec![(OP_BINARY, "bar")],
        },
        Case {
            conf: json!({"open_message": "bar", "open_message_type": "text"}),
            want: vec![(OP_TEXT, "bar")],
        },
        Case {
            conf: json!({"open_message": "bar"}),
            want: vec![(OP_BINARY, "bar")],
        },
        Case {
            conf: json!({"open_messages": ["bar", "baz", "buz"], "open_message_type": "text"}),
            want: vec![(OP_TEXT, "bar"), (OP_TEXT, "baz"), (OP_TEXT, "buz")],
        },
    ];
    for case in cases {
        let (url, mut conns) = ws_server().await;
        let (reg, ctx, _stop) = context(true, Environment::new());
        let mut conf = case.conf.clone();
        conf["url"] = json!(url);
        let mut input = build(conf, &ctx, &reg);
        input.connect(&ctx).await.expect("connects");
        let mut server = accepted(&mut conns).await;
        for (op, text) in &case.want {
            let (got_op, got) = server.recv().await;
            assert_eq!(
                (got_op, got.as_slice()),
                (*op, text.as_bytes()),
                "{}",
                case.conf
            );
        }
        server.send(OP_TEXT, b"foo").await;
        let (payload, ack) = read_payload(&mut input, &ctx).await;
        assert_eq!(payload, b"foo");
        ack.ack();
        input.close().await.unwrap();
    }

    // An unknown open_message_type fails the connect with Bento's text.
    let (url, _conns) = ws_server().await;
    let (reg, ctx, _stop) = context(true, Environment::new());
    let mut input = build(
        json!({"url": url, "open_message": "bar", "open_message_type": "foobar"}),
        &ctx,
        &reg,
    );
    let err = input.connect(&ctx).await.expect_err("refused");
    assert!(
        err.to_string()
            .contains("unrecognised open_message_type: foobar"),
        "{err}"
    );
}

/// A header case: name, the configured headers, and the headers the server must see.
type HeaderCase = (&'static str, Value, Vec<(&'static str, &'static str)>);

#[tokio::test]
async fn bento_input_websocket_test_websocket_headers() {
    let cases: [HeaderCase; 3] = [
        (
            "static",
            json!({"X-Static-Header": "static-value", "Origin": "http://example.com"}),
            vec![
                ("x-static-header", "static-value"),
                ("origin", "http://example.com"),
            ],
        ),
        (
            "interpolated",
            json!({"X-Dynamic-Header": "${! env(\"TEST_HEADER_VALUE\") }"}),
            vec![("x-dynamic-header", "dynamic-value")],
        ),
        (
            "mixed",
            json!({"X-Static": "static", "X-Dynamic": "prefix-${! env(\"TEST_HEADER_VALUE\") }"}),
            vec![
                ("x-static", "static"),
                ("x-dynamic", "prefix-dynamic-value"),
            ],
        ),
    ];
    for (name, headers, want) in cases {
        let (url, mut conns) = ws_server().await;
        let env = Environment::new().with_env_vars(HashMap::from([(
            "TEST_HEADER_VALUE".to_string(),
            "dynamic-value".to_string(),
        )]));
        let (reg, ctx, _stop) = context(true, env);
        let mut input = build(json!({"url": url, "headers": headers}), &ctx, &reg);
        input.connect(&ctx).await.expect("connects");
        let server = accepted(&mut conns).await;
        for (k, v) in want {
            assert_eq!(
                server.headers.get(k).map(String::as_str),
                Some(v),
                "{name}: {k}"
            );
        }
        input.close().await.unwrap();
    }
}

#[tokio::test]
async fn bento_input_websocket_test_websocket_close() {
    let (url, mut conns) = ws_server().await;
    let (reg, ctx, _stop) = context(true, Environment::new());
    let mut input = build(json!({"url": url}), &ctx, &reg);
    input.connect(&ctx).await.expect("connects");
    let mut server = accepted(&mut conns).await;
    input.close().await.unwrap();
    let (op, _) = server.recv().await;
    assert_eq!(op, OP_CLOSE, "close sends a close frame");
    let err = input.read_batch(&ctx).await.err().expect("closed");
    assert!(matches!(err, CoreError::NotConnected), "{err}");
}

#[tokio::test]
async fn loopback_is_refused_unless_the_organization_allows_private_ips() {
    let (url, mut conns) = ws_server().await;
    let (reg, ctx, _stop) = context(false, Environment::new());
    let mut input = build(json!({"url": url}), &ctx, &reg);
    let err = input.connect(&ctx).await.expect_err("loopback is refused");
    assert!(err.to_string().contains("loopback"), "{err}");
    assert!(conns.try_recv().is_err(), "no connection was made");

    let mut by_name = build(json!({"url": "ws://localhost:1/ws"}), &ctx, &reg);
    let err = by_name
        .connect(&ctx)
        .await
        .expect_err("localhost is refused");
    assert!(err.to_string().contains("loopback"), "{err}");
}

#[tokio::test]
async fn a_nacked_message_is_replayed_and_a_lost_connection_reconnects() {
    let (url, mut conns) = ws_server().await;
    let (reg, ctx, _stop) = context(true, Environment::new());
    let mut input = build(json!({"url": url}), &ctx, &reg);
    input.connect(&ctx).await.expect("connects");
    let mut server = accepted(&mut conns).await;
    server.send(OP_TEXT, b"first").await;
    let (payload, ack) = read_payload(&mut input, &ctx).await;
    assert_eq!(payload, b"first");
    ack.nack(BatchError::new(Vec::new(), "output failed"));
    let (payload, ack) = read_payload(&mut input, &ctx).await;
    assert_eq!(payload, b"first", "auto_replay_nacks replays it");
    ack.ack();

    // The server goes away; the next read reconnects and goes on.
    drop(server);
    let reader = tokio::spawn(async move {
        let got = read_payload(&mut input, &ctx).await;
        (got.0, input, ctx)
    });
    let mut second = accepted(&mut conns).await;
    second.send(OP_TEXT, b"after").await;
    let (payload, _input, _ctx) = tokio::time::timeout(Duration::from_secs(15), reader)
        .await
        .expect("read after reconnect")
        .unwrap();
    assert_eq!(payload, b"after");
}

#[tokio::test]
async fn without_auto_replay_a_nack_is_not_replayed() {
    let (url, mut conns) = ws_server().await;
    let (reg, ctx, _stop) = context(true, Environment::new());
    let mut input = build(json!({"url": url, "auto_replay_nacks": false}), &ctx, &reg);
    input.connect(&ctx).await.expect("connects");
    let mut server = accepted(&mut conns).await;
    server.send(OP_TEXT, b"one").await;
    server.send(OP_TEXT, b"two").await;
    let (_, ack) = read_payload(&mut input, &ctx).await;
    ack.nack(BatchError::new(Vec::new(), "failed"));
    let (payload, _) = read_payload(&mut input, &ctx).await;
    assert_eq!(payload, b"two");
}

#[tokio::test]
async fn basic_auth_is_sent_and_never_shown() {
    let (url, mut conns) = ws_server().await;
    let (reg, ctx, _stop) = context(true, Environment::new());
    let conf = json!({"url": url, "basic_auth": {"enabled": true, "username": "u", "password": "ws-e3-pass"}});
    let parsed = WebsocketConf::from_json(&conf, &Environment::new()).unwrap();
    assert!(!format!("{parsed:?}").contains("ws-e3-pass"));
    let mut input = build(conf, &ctx, &reg);
    input.connect(&ctx).await.expect("connects");
    let server = accepted(&mut conns).await;
    let want = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode("u:ws-e3-pass")
    );
    assert_eq!(server.headers.get("authorization"), Some(&want));
}

#[test]
fn oauth1_signs_deterministically_and_keeps_secrets_out_of_the_header() {
    let url = url::Url::parse("wss://stream.example.com/1/statuses?track=rust").unwrap();
    let a = oauth1_header(
        &url,
        "ck",
        "cs-secret",
        "tok",
        "ts-secret",
        "nonce1",
        1_700_000_000,
    );
    let b = oauth1_header(
        &url,
        "ck",
        "cs-secret",
        "tok",
        "ts-secret",
        "nonce1",
        1_700_000_000,
    );
    assert_eq!(a, b);
    assert!(a.starts_with("OAuth oauth_consumer_key=\"ck\""), "{a}");
    assert!(a.contains("oauth_signature_method=\"HMAC-SHA1\""));
    assert!(a.contains("oauth_timestamp=\"1700000000\""));
    assert!(!a.contains("cs-secret") && !a.contains("ts-secret"));
    let c = oauth1_header(
        &url,
        "ck",
        "other",
        "tok",
        "ts-secret",
        "nonce1",
        1_700_000_000,
    );
    assert_ne!(a, c, "the signature depends on the secret");
}

#[test]
fn fields_outside_the_outbound_rule_are_refused() {
    let env = Environment::new();
    let base = |extra: Value| {
        let mut v = json!({"url": "wss://example.com/ws"});
        if let (Some(m), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            for (k, x) in e {
                m.insert(k.clone(), x.clone());
            }
        }
        v
    };
    for (extra, needle) in [
        (json!({"proxy_url": "http://proxy:3128"}), "proxy_url"),
        (json!({"jwt": {"enabled": true}}), "jwt"),
        (
            json!({"tls": {"enabled": true, "skip_cert_verify": true}}),
            "skip_cert_verify",
        ),
        (
            json!({"tls": {"root_cas_file": "/etc/ca.pem"}}),
            "root_cas_file",
        ),
        (
            json!({"tls": {"client_certs": [{"cert": "x", "key": "y"}]}}),
            "client_certs",
        ),
        (json!({"tls": {"root_cas": "not a pem"}}), "root_cas"),
        (json!({"max_message_bytes": 0}), "max_message_bytes"),
    ] {
        let err = WebsocketConf::from_json(&base(extra.clone()), &env).expect_err("refused");
        assert!(err.to_string().contains(needle), "{extra}: {err}");
    }
    let err = WebsocketConf::from_json(
        &json!({"url": "ws://x/ws", "open_message": "a", "open_messages": ["b"]}),
        &env,
    )
    .expect_err("both");
    assert!(
        err.to_string()
            .contains("both open_message and open_messages cannot be set"),
        "{err}"
    );
    assert!(WebsocketConf::from_json(&json!({"url": "http://x/ws"}), &env).is_err());
    assert!(WebsocketConf::from_json(&json!({}), &env).is_err());
    let ok = WebsocketConf::from_json(&json!({"url": "ws://x/ws"}), &env).unwrap();
    assert!(ok.auto_replay_nacks, "Bento's default");
}
