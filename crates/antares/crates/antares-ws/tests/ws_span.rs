// SPDX-License-Identifier: EUPL-1.2
//! A socket's session logs in the span of the request that opened it: its
//! lines carry that request's id, and a host's quiet span stays quiet. A
//! process of its own: tracing caches each callsite's interest per process.

use antares_ops::test_kit::TestBroker;
use antares_ws::{Hub, Limits, Sink, Ws};
use futures_util::{SinkExt, StreamExt};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tracing::Instrument;

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

/// The close line, written by the session task after the upgrade answered,
/// names the span the upgrade request ran in, and so does the delete of the
/// socket's Subscriptions once its resume grace ran out.
#[tokio::test]
async fn the_session_logs_in_the_span_of_its_upgrade_request() -> Result<(), String> {
    let lines = Lines::default();
    let writer = lines.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let hub = Hub::new(Limits {
        resume_grace: Duration::from_millis(50),
        ..Limits::default()
    });
    let broker = TestBroker::with_sinks(vec![Box::new(Sink::new(hub.clone()))], None).await?;
    let app = broker
        .serve_mounted(vec![antares_ops::mount(Ws::new(hub.clone()))])?
        .layer(axum::middleware::from_fn(
            |req: axum::extract::Request, next: axum::middleware::Next| async move {
                let span = tracing::info_span!("upgrade", rid = "ws-span-0001");
                next.run(req).instrument(span).await
            },
        ));
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
        .insert("NGSILD-Tenant", HeaderValue::from_static("city"));
    let (mut ws, _) = tokio_tungstenite::connect_async(req)
        .await
        .map_err(|e| e.to_string())?;
    let _welcome = ws.next().await;
    let subscribe = r#"{"metadata": {"x-op": "x-subscribe", "x-id": "1"},
        "body": {"type": "Subscription", "entities": [{"type": "Car"}]}}"#;
    ws.send(subscribe.into()).await.map_err(|e| e.to_string())?;
    let _made = ws.next().await;
    ws.close(None).await.map_err(|e| e.to_string())?;
    for _ in 0..500 {
        if lines.text().contains("delete_subscription") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let text = lines.text();
    for what in ["websocket closed", "delete_subscription"] {
        let line = text
            .lines()
            .find(|l| l.contains(what))
            .ok_or(format!("no {what} line in {text}"))?;
        assert!(line.contains("upgrade{rid=\"ws-span-0001\"}"), "{line}");
    }
    Ok(())
}
