// SPDX-License-Identifier: EUPL-1.2
//! Rule 10: the open and close lines of a socket. A process of its own:
//! tracing caches each callsite's interest per process, so a subscriber set
//! for one test of a shared binary can miss a line another test registered.

use antares_ops::test_kit::TestBroker;
use antares_ws::{Hub, Limits, Sink, Ws};
use futures_util::StreamExt;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

/// A writer the log lines of this test go to.
#[derive(Clone, Default)]
struct Lines(Arc<Mutex<Vec<u8>>>);

impl Lines {
    fn text(&self) -> String {
        self.0
            .lock()
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .unwrap_or_default()
    }
}

impl std::io::Write for Lines {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Ok(mut v) = self.0.lock() {
            v.extend_from_slice(buf);
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// One line when a socket opens and one when it closes, each naming the
/// connection and the tenant, neither naming the subject. One thread, so
/// the server's tasks log to this test's subscriber.
#[tokio::test]
async fn ws_open_and_close_are_logged_without_the_subject() -> Result<(), String> {
    const SUBJECT: &str = "alice-7d1f-subject";
    std::env::set_var("ANTARES_POLICY_SUBJECT_HEADERS", "x-user");
    let lines = Lines::default();
    let writer = lines.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let hub = Hub::new(Limits::default());
    let broker = TestBroker::with_sinks(vec![Box::new(Sink::new(hub.clone()))], None).await?;
    let app = broker.serve_mounted(vec![antares_ops::mount(Ws::new(hub.clone()))])?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| e.to_string())?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    tokio::spawn(async move {
        let _served = axum::serve(listener, app).await;
    });
    let mut req = format!("ws://{addr}/x/ws/ngsi-ld/v1")
        .into_client_request()
        .map_err(|e| e.to_string())?;
    req.headers_mut()
        .insert("x-user", HeaderValue::from_static(SUBJECT));
    req.headers_mut()
        .insert("NGSILD-Tenant", HeaderValue::from_static("city"));
    let (mut ws, _) = tokio_tungstenite::connect_async(req)
        .await
        .map_err(|e| e.to_string())?;
    let welcome = match ws.next().await {
        Some(Ok(Message::Text(t))) => t.to_string(),
        other => return Err(format!("no welcome: {other:?}")),
    };
    let connection = serde_json::from_str::<serde_json::Value>(&welcome)
        .ok()
        .and_then(|v| {
            v.pointer("/metadata/x-connection")?
                .as_str()
                .map(str::to_owned)
        })
        .ok_or(format!("no connection in {welcome}"))?;
    ws.close(None).await.map_err(|e| e.to_string())?;
    for _ in 0..500 {
        if lines.text().contains("websocket closed") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let text = lines.text();
    for what in ["websocket opened", "websocket closed"] {
        let line = text
            .lines()
            .find(|l| l.contains(what))
            .ok_or(format!("no {what:?} line in {text}"))?;
        assert!(line.contains(&connection), "{line}");
        assert!(line.contains("tenant=city"), "{line}");
    }
    assert!(!text.contains(SUBJECT), "the subject was logged: {text}");
    Ok(())
}
