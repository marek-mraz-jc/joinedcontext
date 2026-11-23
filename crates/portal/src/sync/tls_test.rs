//! An https origin on this host for the sync hops' tests: a TLS front with the test certificate
//! (CN localhost, SAN 127.0.0.1; its key is the constant `[7; 32]`, no secret) before any plain
//! server, and a client that trusts that certificate. The production clients resolve public
//! addresses only, so a test client differs from them in its resolver and its root alone.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio_rustls::rustls;

const TEST_CERT: &[u8] = include_bytes!("../../tests/fixtures/db-relay.cert.der");

/// `https://localhost:{port}` of a TLS front that hands every connection to `behind`.
pub(crate) async fn front(behind: SocketAddr) -> String {
    use p256::pkcs8::EncodePrivateKey;
    let key = p256::SecretKey::from_slice(&[7; 32])
        .expect("a scalar")
        .to_pkcs8_der()
        .expect("an encoded key")
        .as_bytes()
        .to_vec();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("TLS versions")
    .with_no_client_auth()
    .with_single_cert(
        vec![TEST_CERT.to_vec().into()],
        rustls::pki_types::PrivatePkcs8KeyDer::from(key).into(),
    )
    .expect("the test certificate");
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("address").port();
    tokio::spawn(async move {
        while let Ok((client, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let mut client = acceptor.accept(client).await?;
                let mut server = tokio::net::TcpStream::connect(behind).await?;
                tokio::io::copy_bidirectional(&mut client, &mut server).await
            });
        }
    });
    format!("https://localhost:{port}")
}

/// A blocking client like the sync hops' own, trusting the test certificate.
pub(crate) fn client(timeout: Duration, max_redirects: usize) -> reqwest::blocking::Client {
    super::remote::off_runtime(|| {
        reqwest::blocking::Client::builder()
            .no_proxy()
            // The front listens on IPv4 alone; port 0 here keeps the URL's own port.
            .resolve("localhost", SocketAddr::from(([127, 0, 0, 1], 0)))
            .redirect(super::guard::https_redirects(max_redirects))
            .add_root_certificate(
                reqwest::Certificate::from_der(TEST_CERT).expect("the test certificate"),
            )
            .timeout(timeout)
            .build()
    })
    .expect("the test client")
}

/// A plain server that takes connections and never answers, for the timeout of a hop.
pub(crate) async fn silent() -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let at = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });
    at
}

/// A bound socket that does not listen: a connection to its port is refused, and no parallel
/// test can take the port while it is held (no SO_REUSEADDR).
pub(crate) fn closed() -> tokio::net::TcpSocket {
    let socket = tokio::net::TcpSocket::new_v4().expect("a socket");
    socket
        .bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .expect("bind");
    socket
}
