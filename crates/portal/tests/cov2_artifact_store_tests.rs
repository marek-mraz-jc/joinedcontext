//! A store that sends its headers and drops the connection mid-body: every read of an answer is
//! a transport error naming the operation, and a move stops at the object it could not read
//! (TEN-15, T-7858).

use joinedcontext_portal::artifact_store::{move_prefix, Client, Error, Settings};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const BUCKET: &str = "artifacts";

const LISTING: &str = "<ListBucketResult><Contents><Key>legacy/a</Key><Size>1</Size>\
     </Contents><IsTruncated>false</IsTruncated></ListBucketResult>";

/// A plain HTTP store on loopback: a listing is answered whole when `listing_whole`, and every
/// other answer promises 1000 bytes, sends a few and closes.
async fn cutting_store(listing_whole: bool) -> Client {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let at = listener.local_addr().expect("address");
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buffer = [0_u8; 4096];
                while !request.windows(4).any(|end| end == b"\r\n\r\n") {
                    let read = socket.read(&mut buffer).await?;
                    if read == 0 {
                        return Ok(());
                    }
                    request.extend_from_slice(&buffer[..read]);
                }
                let listing = String::from_utf8_lossy(&request)
                    .lines()
                    .next()
                    .is_some_and(|line| line.contains("list-type=2"));
                let answer = if listing && listing_whole {
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{LISTING}",
                        LISTING.len()
                    )
                } else {
                    "HTTP/1.1 200 OK\r\ncontent-length: 1000\r\nconnection: close\r\n\r\n<ListB"
                        .to_owned()
                };
                socket.write_all(answer.as_bytes()).await?;
                socket.shutdown().await
            });
        }
    });
    Client::new(Settings {
        endpoint: format!("http://{at}"),
        bucket: BUCKET.to_owned(),
        region: "us-east-1".to_owned(),
        root_access_key: "jc-root".to_owned(),
        root_secret_key: "the-root-secret-nobody-else-holds".to_owned(),
    })
    .expect("a client")
}

#[tokio::test]
async fn an_object_whose_body_is_cut_off_is_a_transport_error_naming_the_read() {
    let client = cutting_store(true).await;
    let said = client
        .object(
            reqwest::Method::GET,
            BUCKET,
            "a",
            Vec::new(),
            &[],
            "read an object",
        )
        .await
        .expect_err("cut off")
        .to_string();
    assert!(said.contains("read an object did not reach"), "{said}");
}

#[tokio::test]
async fn a_listing_whose_body_is_cut_off_is_a_transport_error_of_the_listing() {
    let client = cutting_store(false).await;
    assert!(matches!(
        client.list(BUCKET, "legacy/").await,
        Err(Error::Transport {
            operation: "list objects",
            ..
        })
    ));
}

#[tokio::test]
async fn a_move_stops_at_an_old_object_it_could_not_read() {
    let client = cutting_store(true).await;
    let said = move_prefix(
        &client,
        BUCKET,
        ("legacy/", "keyed/"),
        "moved/rollback.json",
        serde_json::Map::new(),
    )
    .await
    .expect_err("the read is cut off");
    assert!(said.contains("read an object did not reach"), "{said}");
}
