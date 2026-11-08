//! jc hands the Portal plain loopback URLs of its own listener for the sibling calls
//! (`http://127.0.0.1:{port}{prefix}`). Those go through the Portal's plain clients only. Every
//! fetch of a URL somebody typed (a SyncSource, a peer schema, an event link, an App fetch) goes
//! through `jc_core::outbound`, which must keep refusing the same listener: jc adds no
//! exception to it.

use jc_core::outbound::{self, Refusal};
use std::time::Duration;
use tokio::net::TcpListener;

#[tokio::test]
async fn a_guarded_fetch_cannot_reach_jc_s_own_listener() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let client = outbound::client(Duration::from_secs(2), Duration::from_secs(4))
        .build()
        .expect("client");
    for prefix in [
        "/opendata/api/3/action/organization_create",
        "/streams/streams",
        "/git/api/v1/repos",
        "/broker/ngsi-ld/v1/entities",
    ] {
        for host in ["127.0.0.1", "[::1]", "0.0.0.0"] {
            let url = reqwest::Url::parse(&format!("http://{host}:{port}{prefix}")).expect("url");
            assert!(
                matches!(outbound::check(&url), Err(Refusal::Private(_))),
                "{url} passed the address check"
            );
        }
        let named = format!("http://localhost:{port}{prefix}");
        let err = client
            .get(&named)
            .send()
            .await
            .expect_err("localhost was fetched");
        assert!(outbound::refused(&err).is_some(), "{named}: {err}");
    }
    let accepted = tokio::time::timeout(Duration::from_millis(300), listener.accept()).await;
    assert!(
        accepted.is_err(),
        "the guarded client opened a connection to jc's listener"
    );
}
