// SPDX-License-Identifier: EUPL-1.2
//! The Organization check at the WebSocket door (EP-188, T-7042): under
//! `ANTARES_TENANT_ORGANIZATION=enforce` an upgrade naming another
//! Organization's Tenant gets the answer an unknown Tenant gets, byte for
//! byte. A process of its own: it names the subject headers once.

use antares_ops::test_kit::TestBroker;
use antares_ws::{Hub, Limits, Ws};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Error;
use tower::ServiceExt;

const MEMBER_OF_B: [(&str, &str); 3] = [
    ("x-jc-subject", "someone"),
    ("x-jc-member", "true"),
    ("x-jc-organization-id", "sk_b"),
];
const MEMBER_OF_A: [(&str, &str); 3] = [
    ("x-jc-subject", "someone"),
    ("x-jc-member", "true"),
    ("x-jc-organization-id", "sk_a"),
];
const ROOM: &str = r#"{"id": "urn:ngsi-ld:Room:1", "type": "Room"}"#;

/// The upgrade's answer: `None` once the socket opened, else the refused
/// status and body.
async fn upgrade(
    addr: std::net::SocketAddr,
    caller: &[(&'static str, &'static str)],
) -> Result<Option<(u16, Vec<u8>)>, String> {
    let mut req = format!("ws://{addr}/x/ws/ngsi-ld/v1")
        .into_client_request()
        .map_err(|e| e.to_string())?;
    req.headers_mut()
        .insert("NGSILD-Tenant", HeaderValue::from_static("sk_b__x"));
    for (k, v) in caller {
        req.headers_mut().insert(*k, HeaderValue::from_static(v));
    }
    match tokio_tungstenite::connect_async(req).await {
        Ok(_) => Ok(None),
        Err(Error::Http(resp)) => Ok(Some((
            resp.status().as_u16(),
            resp.body().clone().unwrap_or_default(),
        ))),
        Err(e) => Err(format!("upgrade: {e}")),
    }
}

#[tokio::test]
async fn an_upgrade_on_another_organizations_tenant_reads_as_an_unknown_tenant(
) -> Result<(), String> {
    TestBroker::subject_headers("x-jc-subject,x-jc-member");
    let broker = TestBroker::enforcing_tenant_organization().await?;
    let app = broker.serve_mounted(vec![antares_ops::mount(Ws::new(Hub::new(
        Limits::default(),
    )))])?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| e.to_string())?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let served = app.clone();
    tokio::spawn(async move {
        let _served = axum::serve(listener, served).await;
    });

    // Before `sk_b__x` exists: the unknown-Tenant answer, to the owner too.
    let unknown = upgrade(addr, &MEMBER_OF_B)
        .await?
        .ok_or("an unknown Tenant is refused under enforce")?;
    assert_eq!(unknown.0, 404);
    assert_eq!(
        upgrade(addr, &MEMBER_OF_A).await?,
        Some(unknown.clone()),
        "before it exists"
    );

    let mut create = axum::http::Request::post("/ngsi-ld/v1/entities")
        .header("NGSILD-Tenant", "sk_b__x")
        .header("Content-Type", "application/json")
        .header("Content-Length", ROOM.len());
    for (k, v) in MEMBER_OF_B {
        create = create.header(k, v);
    }
    let Ok(made) = app
        .oneshot(
            create
                .body(axum::body::Body::from(ROOM))
                .map_err(|e| e.to_string())?,
        )
        .await;
    assert_eq!(made.status(), axum::http::StatusCode::CREATED);

    assert_eq!(upgrade(addr, &MEMBER_OF_B).await?, None, "the owner opens");
    assert_eq!(
        upgrade(addr, &MEMBER_OF_A).await?,
        Some(unknown),
        "another Organization's Tenant answers as an unknown one, byte for byte"
    );
    Ok(())
}
