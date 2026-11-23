//! The client against the scripted broker: metadata, seeds, framing refusals, SASL and TLS.

use base64::Engine;
use ekiben_core::{OrgQuotas, StreamContext};

use std::sync::Arc;
use std::time::Duration;

use ekiben_core::redact::SecretString;
use ekiben_core::CoreError;

use crate::client::*;
use crate::scram;
use crate::tests::ctx;
use crate::tests::fake::{self, Fake, Hook, Reply, Topic, SASL_AUTHENTICATE, SASL_HANDSHAKE};

const T: Duration = Duration::from_secs(5);
const SALT: &[u8] = b"salt-of-the-test";

pub(crate) fn org() -> StreamContext {
    ctx(OrgQuotas {
        allow_private_ips: true,
        ..Default::default()
    })
}

fn settings(c: &StreamContext) -> ConnSettings {
    ConnSettings {
        client_id: "ekiben-test".into(),
        tls: None,
        sasl: None,
        max_response: 1 << 20,
        request_timeout: T,
        org: c.org.clone(),
    }
}

fn sasl(mechanism: Mechanism, password: &str) -> Option<Sasl> {
    Some(Sasl {
        mechanism,
        username: "ingest".into(),
        password: SecretString::new(password),
    })
}

fn error<T>(r: Result<T, CoreError>) -> CoreError {
    r.err().expect("refused")
}

/// A port nothing listens on: bound and never listening, so no parallel test takes it.
fn closed_port() -> (tokio::net::TcpSocket, u16) {
    let s = tokio::net::TcpSocket::new_v4().expect("socket");
    s.bind("127.0.0.1:0".parse().expect("addr")).expect("bind");
    let port = s.local_addr().expect("addr").port();
    (s, port)
}

/// A broker that answers SASL as `mechanism` with `password` and leaves every other call to the
/// model.
pub(crate) fn scram_broker(mechanism: Mechanism, password: &'static str) -> Hook {
    let mut first: Option<(String, String)> = None;
    Box::new(move |api, body| match api {
        SASL_HANDSHAKE => Some(fake::handshake(0, &[mechanism.name()])),
        SASL_AUTHENTICATE => {
            let msg = String::from_utf8(fake::sasl_bytes(body)).unwrap_or_default();
            let last = match first.take() {
                None => {
                    let bare = msg.trim_start_matches("n,,").to_string();
                    let nonce = bare.rsplit("r=").next().unwrap_or_default();
                    let b64 = base64::engine::general_purpose::STANDARD.encode(SALT);
                    let sf = format!("r={nonce}server,s={b64},i=4096");
                    first = Some((bare, sf.clone()));
                    sf
                }
                Some((bare, sf)) => {
                    scram::server_final(mechanism, password, SALT, &bare, &sf, &msg)
                }
            };
            Some(fake::authenticated(0, None, last.as_bytes()))
        }
        _ => None,
    })
}

/// A certificate for `localhost`, its DER as `tls.root_cas` gives it, and the broker's acceptor.
pub(crate) fn localhost_tls() -> (String, Vec<u8>, tokio_rustls::TlsAcceptor) {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).expect("cert");
    let key = rustls_pki_types::PrivateKeyDer::Pkcs8(cert.signing_key.serialize_der().into());
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("versions")
    .with_no_client_auth()
    .with_single_cert(vec![cert.cert.der().clone()], key)
    .expect("server config");
    (
        cert.cert.pem(),
        cert.cert.der().to_vec(),
        tokio_rustls::TlsAcceptor::from(Arc::new(config)),
    )
}

#[tokio::test]
async fn metadata_keeps_led_partitions_and_names_refused_topics() {
    let f = Fake::start().await;
    f.lock().topics = vec![
        Topic {
            name: "t".into(),
            code: 0,
            // partition 1 has no leader, partition 2 answers an error
            parts: vec![(0, 0, 1), (0, 1, -1), (5, 2, 1)],
        },
        Topic {
            name: "secret".into(),
            code: 29,
            parts: Vec::new(),
        },
        Topic {
            name: "other".into(),
            code: 0,
            parts: vec![(0, 0, 1)],
        },
    ];
    let c = org();
    let mut cl = Cluster::new(settings(&c), vec![("127.0.0.1".into(), f.port)]);
    cl.refresh(Some(&["t".into(), "secret".into()]))
        .await
        .expect("metadata");
    assert_eq!(cl.meta.topics.get("t"), Some(&vec![(0, 1)]));
    assert!(
        !cl.meta.topics.contains_key("other"),
        "only the topics asked for"
    );
    assert_eq!(
        cl.meta.errors,
        vec![("secret".to_string(), "TOPIC_AUTHORIZATION_FAILED")]
    );
    assert_eq!(
        cl.meta.brokers.get(&1),
        Some(&Broker {
            host: "127.0.0.1".into(),
            port: f.port
        })
    );
}

#[tokio::test]
async fn a_broker_port_past_65535_is_a_malformed_answer() {
    let f = Fake::start().await;
    f.lock().brokers = vec![(1, "127.0.0.1".into(), 70_000)];
    let c = org();
    let mut cl = Cluster::new(settings(&c), vec![("127.0.0.1".into(), f.port)]);
    let e = error(cl.refresh(None).await).to_string();
    assert!(
        e.contains("malformed Kafka response: a broker port is out of range"),
        "{e}"
    );
}

#[tokio::test]
async fn the_next_seed_answers_when_the_first_is_down() {
    let f = Fake::start().await;
    let (_held, down) = closed_port();
    let c = org();
    let mut cl = Cluster::new(
        settings(&c),
        vec![("127.0.0.1".into(), down), ("127.0.0.1".into(), f.port)],
    );
    cl.refresh(None).await.expect("the second seed answers");
    assert!(cl.meta.topics.contains_key("t"));

    let mut cl = Cluster::new(settings(&c), vec![("127.0.0.1".into(), down)]);
    let e = error(cl.refresh(None).await).to_string();
    assert!(e.contains(&format!("broker 127.0.0.1:{down}")), "{e}");
}

#[tokio::test]
async fn a_node_outside_the_metadata_is_not_dialled() {
    let c = org();
    let mut cl = Cluster::new(settings(&c), Vec::new());
    let e = error(cl.call(9, METADATA, 4, &[], T).await).to_string();
    assert!(e.contains("broker 9 is not in the metadata"), "{e}");
    let e = error(cl.refresh(None).await).to_string();
    assert!(e.contains("no seed broker"), "{e}");
}

/// Each way a broker can answer badly is refused with what was wrong, never read past.
#[tokio::test]
async fn malformed_and_missing_answers_are_refused() {
    let mut s = settings(&org());
    s.max_response = 1024;
    s.request_timeout = Duration::from_millis(300);
    let cases: Vec<(Reply, &str)> = vec![
        (
            Reply::Raw((-5i32).to_be_bytes().to_vec(), false),
            "a response size is negative",
        ),
        (
            Reply::Raw(4096i32.to_be_bytes().to_vec(), false),
            "a response of 4096 bytes is over broker_read_max_bytes (1024 bytes)",
        ),
        (
            Reply::Raw(2i32.to_be_bytes().to_vec(), false),
            "a response has no header",
        ),
        (
            Reply::Raw([4i32.to_be_bytes(), 999i32.to_be_bytes()].concat(), false),
            "a response answers another request",
        ),
        // half a body, then nothing
        (
            Reply::Raw([8i32.to_be_bytes(), 1i32.to_be_bytes()].concat(), false),
            "the broker did not answer in time",
        ),
        // half a body, then the connection closes
        (
            Reply::Raw([8i32.to_be_bytes(), 1i32.to_be_bytes()].concat(), true),
            "connection: ",
        ),
        (Reply::Close, "connection: "),
        (Reply::Stall, "the broker did not answer in time"),
    ];
    for (reply, want) in cases {
        let f = Fake::start().await;
        f.push(METADATA, reply);
        let mut conn = Conn::connect("127.0.0.1", f.port, &s)
            .await
            .expect("connect");
        let e = error(conn.call(METADATA, 4, &[], &s, s.request_timeout).await).to_string();
        assert!(e.contains(want), "{want}: {e}");
    }
}

#[tokio::test]
async fn a_failed_call_drops_the_connection_and_the_next_call_dials_again() {
    let f = Fake::start().await;
    let c = org();
    let mut cl = Cluster::new(settings(&c), vec![("127.0.0.1".into(), f.port)]);
    cl.refresh(None).await.expect("metadata");
    f.push(METADATA, Reply::Close);
    error(cl.call(1, METADATA, 4, &[255, 255, 255, 255, 0], T).await);
    let d = cl
        .call(1, METADATA, 4, &[255, 255, 255, 255, 0], T)
        .await
        .expect("redialled");
    assert!(decode_metadata(d)
        .expect("metadata")
        .topics
        .contains_key("t"));
}

#[tokio::test]
async fn sasl_plain_sends_the_user_and_password_once() {
    let f = Fake::start().await;
    f.push(SASL_HANDSHAKE, fake::handshake(0, &["PLAIN"]));
    f.push(SASL_AUTHENTICATE, fake::authenticated(0, None, b""));
    let mut s = settings(&org());
    s.sasl = sasl(Mechanism::Plain, "hunter2");
    let mut conn = Conn::connect("127.0.0.1", f.port, &s)
        .await
        .expect("authenticated");
    conn.call(METADATA, 4, &[255, 255, 255, 255, 0], &s, T)
        .await
        .expect("the connection is usable");
    let hs = f.seen(SASL_HANDSHAKE);
    assert_eq!(&hs[0][..], b"\x00\x05PLAIN");
    assert_eq!(
        fake::sasl_bytes(&f.seen(SASL_AUTHENTICATE)[0]),
        b"\0ingest\0hunter2"
    );
}

#[tokio::test]
async fn a_mechanism_the_broker_does_not_offer_names_the_ones_it_does() {
    let f = Fake::start().await;
    f.push(
        SASL_HANDSHAKE,
        fake::handshake(33, &["SCRAM-SHA-512", "PLAIN"]),
    );
    let mut s = settings(&org());
    s.sasl = sasl(Mechanism::ScramSha256, "hunter2");
    let e = error(Conn::connect("127.0.0.1", f.port, &s).await);
    assert!(matches!(e, CoreError::Config(_)), "{e:?}");
    assert!(
        e.to_string().contains("does not offer SASL SCRAM-SHA-256 (UNSUPPORTED_SASL_MECHANISM); it offers: SCRAM-SHA-512, PLAIN"),
        "{e}"
    );
}

#[tokio::test]
async fn a_refused_login_names_the_broker_s_reason_and_not_the_password() {
    let f = Fake::start().await;
    f.push(SASL_HANDSHAKE, fake::handshake(0, &["PLAIN"]));
    f.push(
        SASL_AUTHENTICATE,
        fake::authenticated(58, Some("Authentication failed"), b""),
    );
    let mut s = settings(&org());
    s.sasl = sasl(Mechanism::Plain, "hunter2");
    let e = error(Conn::connect("127.0.0.1", f.port, &s).await);
    let text = e.to_string();
    assert!(matches!(e, CoreError::Config(_)), "{e:?}");
    assert!(
        text.contains("SASL authentication failed (SASL_AUTHENTICATION_FAILED): Authentication failed; check sasl.username"),
        "{text}"
    );
    assert!(!text.contains("hunter2"), "{text}");
}

#[tokio::test]
async fn scram_sha_256_and_512_log_in_and_check_the_server() {
    for m in [Mechanism::ScramSha256, Mechanism::ScramSha512] {
        let f = Fake::start().await;
        f.lock().hook = Some(scram_broker(m, "pencil"));
        let mut s = settings(&org());
        s.sasl = sasl(m, "pencil");
        let mut conn = Conn::connect("127.0.0.1", f.port, &s)
            .await
            .unwrap_or_else(|e| panic!("{}: {e}", m.name()));
        conn.call(METADATA, 4, &[255, 255, 255, 255, 0], &s, T)
            .await
            .expect("the connection is usable");
        let first =
            String::from_utf8(fake::sasl_bytes(&f.seen(SASL_AUTHENTICATE)[0])).expect("utf-8");
        assert!(first.starts_with("n,,n=ingest,r="), "{first}");
    }
}

#[tokio::test]
async fn scram_with_the_wrong_password_or_a_hostile_server_is_refused() {
    let f = Fake::start().await;
    f.lock().hook = Some(scram_broker(Mechanism::ScramSha512, "pencil"));
    let mut s = settings(&org());
    s.sasl = sasl(Mechanism::ScramSha512, "crayon");
    let e = error(Conn::connect("127.0.0.1", f.port, &s).await).to_string();
    assert!(
        e.contains("SASL SCRAM-SHA-512: the server refused: invalid-proof"),
        "{e}"
    );
    assert!(!e.contains("crayon"), "{e}");

    // a first message whose nonce is not the client's
    let f = Fake::start().await;
    f.push(SASL_HANDSHAKE, fake::handshake(0, &["SCRAM-SHA-256"]));
    f.push(
        SASL_AUTHENTICATE,
        fake::authenticated(0, None, b"r=other,s=AAAA,i=4096"),
    );
    s.sasl = sasl(Mechanism::ScramSha256, "crayon");
    let e = error(Conn::connect("127.0.0.1", f.port, &s).await).to_string();
    assert!(
        e.contains("SASL SCRAM-SHA-256: the server's nonce does not extend the client's"),
        "{e}"
    );
}

#[tokio::test]
async fn tls_verifies_the_broker_against_root_cas() {
    let (_, der, acceptor) = localhost_tls();
    let f = Fake::start_with(Some(acceptor)).await;
    let mut s = settings(&org());
    s.tls = Some(tls_config(&[der]).expect("roots"));
    let mut conn = Conn::connect("localhost", f.port, &s).await.expect("TLS");
    let d = conn
        .call(METADATA, 4, &[255, 255, 255, 255, 0], &s, T)
        .await
        .expect("metadata over TLS");
    assert!(decode_metadata(d)
        .expect("metadata")
        .topics
        .contains_key("t"));

    // the system roots alone do not know the broker's certificate
    s.tls = Some(tls_config(&[]).expect("roots"));
    let e = error(Conn::connect("localhost", f.port, &s).await).to_string();
    assert!(
        e.contains(&format!(
            "broker localhost:{}: TLS handshake failed",
            f.port
        )),
        "{e}"
    );
}

#[test]
fn a_root_ca_that_is_not_a_certificate_is_a_configuration_error() {
    let e = error(tls_config(&[b"not a certificate".to_vec()]));
    assert!(matches!(e, CoreError::Config(_)), "{e:?}");
    assert!(
        e.to_string().contains("kafka_franz input: tls.root_cas: "),
        "{e}"
    );
}

/// A broker that takes the connection and never answers the TLS hello times out.
#[tokio::test(start_paused = true)]
async fn a_silent_tls_handshake_times_out() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        let _held = listener.accept().await;
        std::future::pending::<()>().await;
    });
    let (_, der, _) = localhost_tls();
    let mut s = settings(&org());
    s.tls = Some(tls_config(&[der]).expect("roots"));
    let e = error(Conn::connect("localhost", port, &s).await).to_string();
    assert!(e.contains("TLS handshake timed out"), "{e}");
}
