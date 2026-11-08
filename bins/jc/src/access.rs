//! One request id per request across jc's modules, and one access line per call at a module's
//! door: the edge's job in the cluster (APISIX `request-id` plugin and access log), which jc
//! does without an edge.
//!
//! Each mount takes the caller's `X-Request-Id` when it is a plain token and mints one
//! otherwise, hands it on to the module in the request (so the Portal and the gateway, which
//! read the header, keep it for their own lines and the broker hop), runs the module inside a
//! `request` span that names the module and the id (every line the module writes for the
//! request carries both), answers with the id, and writes one `jc::access` line: method, path
//! without the query, status and milliseconds; for a refusal or a failure also the reason.

use axum::Router;
use axum::extract::Request;
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use std::time::Instant;
use tracing::Instrument;

pub const HEADER: HeaderName = HeaderName::from_static("x-request-id");

/// Letters, digits and dashes, 8 to 64 of them (the Portal's rule): anything else could forge
/// a log line or carry more than a reference.
fn plain(id: &str) -> bool {
    (8..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn fresh() -> String {
    let mut bytes = [0u8; 16];
    // A failed draw leaves zeros: still a well-formed id, never a reason to refuse the call.
    let _ = getrandom::fill(&mut bytes);
    hex::encode(bytes)
}

/// The id a request goes on with: the caller's when it is plain (the first one), else a new one.
pub fn request_id(req: &Request) -> String {
    req.headers()
        .get(&HEADER)
        .and_then(|v| v.to_str().ok())
        .filter(|id| plain(id))
        .map_or_else(fresh, str::to_owned)
}

/// `router` behind the access layer of `module`.
pub fn layer(module: &'static str, router: Router) -> Router {
    router.layer(axum::middleware::from_fn(
        move |req: Request, next: Next| access(module, req, next),
    ))
}

async fn access(module: &'static str, mut req: Request, next: Next) -> Response {
    let id = request_id(&req);
    // `plain` or hex: always a valid header value.
    let value = HeaderValue::from_str(&id).unwrap_or(HeaderValue::from_static("invalid"));
    req.headers_mut().insert(HEADER, value.clone());
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let span = tracing::info_span!("request", module, request_id = %id);
    let started = Instant::now();
    let mut response = next.run(req).instrument(span.clone()).await;
    let status = response.status();
    let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    span.in_scope(|| {
        let code = status.as_u16();
        if status.is_server_error() || status.is_client_error() {
            let reason = status.canonical_reason().unwrap_or("refused");
            tracing::info!(target: "jc::access", module, %method, path, status = code, ms, reason, "refused");
        } else {
            tracing::info!(target: "jc::access", module, %method, path, status = code, ms, "answered");
        }
    });
    response.headers_mut().insert(HEADER, value);
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::StatusCode;
    use axum::routing::get;
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;

    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);

    impl Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .map_err(|_| std::io::ErrorKind::Other)?
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A module that logs one line, refuses `/closed` and answers the id it was handed.
    fn module() -> Router {
        Router::new()
            .route(
                "/open",
                get(|req: Request| async move {
                    tracing::info!(target: "shako_api::x", "inner line");
                    req.headers()
                        .get(&HEADER)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_owned()
                }),
            )
            .route("/closed", get(|| async { StatusCode::UNAUTHORIZED }))
    }

    async fn call(path: &str, id: Option<&str>) -> (Response, String) {
        let buf = Buffer::default();
        let writer = buf.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .finish();
        let mut req = Request::builder().uri(format!("{path}?secret=q"));
        if let Some(id) = id {
            req = req.header("x-request-id", id);
        }
        let req = req.body(Body::empty()).expect("request");
        let app = layer("shako", module());
        let _guard = tracing::subscriber::set_default(subscriber);
        let response = app.oneshot(req).await.expect("answer");
        let log = String::from_utf8(buf.0.lock().expect("log").clone()).expect("utf8");
        (response, log)
    }

    fn echoed(r: &Response) -> &str {
        r.headers()
            .get(&HEADER)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_callers_id_reaches_the_module_its_lines_and_the_answer() {
        let (response, log) = call("/open", Some("smokelogs-0123456789")).await;
        assert_eq!(echoed(&response), "smokelogs-0123456789");
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .expect("body");
        assert_eq!(&body[..], b"smokelogs-0123456789");
        let inner = log
            .lines()
            .find(|l| l.contains("inner line"))
            .expect("inner line");
        assert!(inner.contains("request_id=smokelogs-0123456789"), "{log}");
        assert!(inner.contains("module=\"shako\""), "{log}");
        let access: Vec<_> = log.lines().filter(|l| l.contains("jc::access")).collect();
        assert_eq!(access.len(), 1, "{log}");
        assert!(
            access[0].contains("status=200") && access[0].contains("path=\"/open\""),
            "{log}"
        );
        assert!(
            !log.contains("secret=q"),
            "the query stays out of the log: {log}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_refusal_logs_one_line_with_its_reason() {
        let (response, log) = call("/closed", Some("smokelogs-refused-01")).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let lines: Vec<_> = log
            .lines()
            .filter(|l| l.contains("smokelogs-refused-01"))
            .collect();
        assert_eq!(lines.len(), 1, "{log}");
        assert!(
            lines[0].contains("status=401") && lines[0].contains("reason=\"Unauthorized\""),
            "{log}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_forged_or_missing_id_is_replaced() {
        for sent in [None, Some("a b level=error forged"), Some("short")] {
            let (response, log) = call("/open", sent).await;
            let id = echoed(&response).to_owned();
            assert!(
                id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()),
                "{id}"
            );
            assert!(log.contains(&format!("request_id={id}")), "{log}");
            assert!(!log.contains("forged"), "{log}");
        }
    }
}
