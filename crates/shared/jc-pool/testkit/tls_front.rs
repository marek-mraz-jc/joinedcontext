//! A TLS front of a test Postgres, which has none in CI: the pool hang of T-7494 needs TLS.
//! Included by `#[path]` in the Portal's and the assistant's tests, which hold the TLS crates.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::rustls;

/// The URL of the server of `url` through a TLS front serving `cert` and `key`.
pub async fn tls_url(url: &str, cert: Vec<u8>, key: Vec<u8>) -> String {
    let options = sqlx::postgres::PgConnectOptions::from_str(url).expect("a URL");
    let server = (options.get_host().to_owned(), options.get_port());
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("TLS versions")
    .with_no_client_auth()
    .with_single_cert(
        vec![cert.into()],
        rustls::pki_types::PrivatePkcs8KeyDer::from(key).into(),
    )
    .expect("the test certificate");
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let front = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = front.local_addr().expect("address").port();
    tokio::spawn(async move {
        while let Ok((mut client, _)) = front.accept().await {
            let (acceptor, server) = (acceptor.clone(), server.clone());
            tokio::spawn(async move {
                // Postgres's SSLRequest, answered `S` before the handshake.
                let mut request = [0; 8];
                client.read_exact(&mut request).await?;
                client.write_all(b"S").await?;
                let client = acceptor.accept(client).await?;
                let server = tokio::net::TcpStream::connect(server).await?;
                let (mut from_client, mut to_client) = tokio::io::split(client);
                let (mut from_server, mut to_server) = server.into_split();
                let up = async {
                    tokio::io::copy(&mut from_client, &mut to_server).await?;
                    to_server.shutdown().await
                };
                let down = async {
                    let mut answer = [0; 16384];
                    loop {
                        let mut read = from_server.read(&mut answer).await?;
                        if read == 0 {
                            return to_client.shutdown().await;
                        }
                        // The cancel (SQLSTATE 57014) and the answers queued behind it go out as
                        // one record: the client has then read all there is, and the close hung.
                        if answer[..read].windows(5).any(|code| code == b"57014") {
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            read += from_server.try_read(&mut answer[read..]).unwrap_or(0);
                        }
                        to_client.write_all(&answer[..read]).await?;
                        to_client.flush().await?;
                    }
                };
                // Either side hanging up ends the connection, as a real server's exit does.
                tokio::select! {
                    done = up => done,
                    done = down => done,
                }
            });
        }
    });
    let mut through = url::Url::parse(url).expect("a URL");
    through.set_host(Some("127.0.0.1")).expect("a host");
    through.set_port(Some(port)).expect("a port");
    through.query_pairs_mut().append_pair("sslmode", "require");
    through.into()
}
