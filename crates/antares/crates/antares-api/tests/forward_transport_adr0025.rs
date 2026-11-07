// SPDX-License-Identifier: EUPL-1.2
//! ADR-0025 rule 8 (as amended by ADR-0027 section 9): every distributed
//! operation reaches the registered `ForwardTransport` with the request the
//! HTTP branch sends, and a transport that answers `None` leaves the HTTP
//! path as it was, byte for byte. One binary: it sets `ANTARES_PUBLIC_URL`
//! for the distributed subscription before the broker reads it.
#![cfg(feature = "test-kit")]
#![allow(clippy::unwrap_used)] // an unwrap here is the assertion

use antares_api::{AppState, BuiltForward, ForwardAnswer, ForwardFuture, ForwardTransport};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tower::ServiceExt;

/// One forward as the transport saw it: method, target, headers, body.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// Records every forward and serves none of them.
#[derive(Default)]
struct Recorder(Mutex<Vec<Seen>>);

impl ForwardTransport for Recorder {
    fn forward<'a>(&'a self, req: BuiltForward<'a>) -> Option<ForwardFuture<'a>> {
        self.0.lock().unwrap().push(Seen {
            method: req.method().to_string(),
            target: req.target().unwrap(),
            headers: req.headers().to_vec(),
            body: req.body().unwrap_or_default().to_vec(),
        });
        None
    }
}

/// A peer that records every request whole (head and body) and answers
/// `200 []`.
fn peer() -> (u16, Arc<Mutex<Vec<String>>>) {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let sink = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                match s.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
            }
            let end = buf
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .map_or(buf.len(), |p| p + 4);
            let head = String::from_utf8_lossy(&buf[..end]).to_string();
            let want: usize = head
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse().unwrap_or(0))
                })
                .unwrap_or(0);
            while buf.len() - end < want {
                match s.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
            }
            sink.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&buf).to_string());
            let _ = s.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]",
            );
        }
    });
    (port, seen)
}

async fn state(transport: Option<Arc<dyn ForwardTransport>>) -> AppState {
    antares_jsonld::allow_private_egress(true);
    std::env::set_var("ANTARES_PUBLIC_URL", "http://127.0.0.1:9999");
    let mut st = AppState::new("me".into());
    antares_api::wire(&mut st).await;
    match transport {
        Some(t) => st.with_forward_transport(t).unwrap(),
        None => st,
    }
}

async fn send(
    st: &AppState,
    method: &str,
    path: &str,
    doc: Option<Value>,
) -> (StatusCode, Vec<String>, Value) {
    let b = Request::builder()
        .method(method)
        .uri(format!("/ngsi-ld/v1/{path}"));
    let req = match doc {
        Some(v) => {
            let payload = v.to_string();
            b.header("Content-Type", "application/json")
                .header("Content-Length", payload.len())
                .body(Body::from(payload))
        }
        None => b.body(Body::empty()),
    }
    .unwrap();
    let res = antares_api::router(st.clone()).oneshot(req).await.unwrap();
    let status = res.status();
    let warnings = res
        .headers()
        .get_all("ngsild-warning")
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .collect();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        warnings,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// One distributed operation: the registration that makes it forward, and
/// the request that triggers it.
struct Case {
    caller: &'static str,
    /// How the forward's request line starts: the caller's own resource.
    line: &'static str,
    mode: &'static str,
    operations: &'static [&'static str],
    method: &'static str,
    path: &'static str,
    doc: Option<Value>,
}

fn vehicle() -> Value {
    json!({"id": "urn:ngsi-ld:Vehicle:9", "type": "Vehicle", "speed": {"type": "Property", "value": 1}})
}

fn cases() -> Vec<Case> {
    let case = |caller, line, mode, operations, method, path, doc| Case {
        caller,
        line,
        mode,
        operations,
        method,
        path,
        doc,
    };
    vec![
        case(
            "fed_retrieve",
            "GET /ngsi-ld/v1/entities/urn:ngsi-ld:Vehicle:9",
            "inclusive",
            &["retrieveEntity"],
            "GET",
            "entities/urn:ngsi-ld:Vehicle:9",
            None,
        ),
        case(
            "fed_query",
            "GET /ngsi-ld/v1/entities?",
            "inclusive",
            &["queryEntity"],
            "GET",
            "entities?type=Vehicle",
            None,
        ),
        case(
            "fed_retrieve_temporal",
            "GET /ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Vehicle:9",
            "inclusive",
            &["retrieveTemporal"],
            "GET",
            "temporal/entities/urn:ngsi-ld:Vehicle:9",
            None,
        ),
        case(
            "fed_query_temporal",
            "GET /ngsi-ld/v1/temporal/entities?",
            "inclusive",
            &["queryTemporal"],
            "GET",
            "temporal/entities?type=Vehicle&timerel=after&timeAt=2020-01-01T00:00:00Z",
            None,
        ),
        case(
            "fed_entity_maps",
            "GET /ngsi-ld/v1/entityMaps?",
            "inclusive",
            &["federationOps"],
            "GET",
            "entityMaps?type=Vehicle",
            None,
        ),
        case(
            "forward_part",
            "POST /ngsi-ld/v1/entities ",
            "redirect",
            &["createEntity"],
            "POST",
            "entities",
            Some(vehicle()),
        ),
        case(
            "forward_batch",
            "POST /ngsi-ld/v1/entityOperations/create ",
            "redirect",
            &["createBatch"],
            "POST",
            "entityOperations/create",
            Some(json!([vehicle()])),
        ),
        case(
            "forward_one",
            "POST /ngsi-ld/v1/entities ",
            "redirect",
            &["createEntity"],
            "POST",
            "entityOperations/create",
            Some(json!([vehicle()])),
        ),
        case(
            "batch_delete",
            "POST /ngsi-ld/v1/entityOperations/delete ",
            "redirect",
            &["deleteBatch"],
            "POST",
            "entityOperations/delete",
            Some(json!(["urn:ngsi-ld:Vehicle:9"])),
        ),
        case(
            "forward_sub",
            "POST /ngsi-ld/v1/subscriptions ",
            "inclusive",
            &["federationOps"],
            "POST",
            "subscriptions",
            Some(json!({
                "id": "urn:ngsi-ld:Subscription:ft",
                "type": "Subscription",
                "entities": [{"type": "Vehicle"}],
                "notification": {"endpoint": {"uri": "http://127.0.0.1:9998/n"}},
            })),
        ),
    ]
}

/// Runs `case` against a fresh broker forwarding to `port`, and returns
/// what `wire` holds once the forwards arrived (a distributed subscription
/// is forwarded after the answer).
async fn run(
    case: &Case,
    port: u16,
    wire: &Arc<Mutex<Vec<String>>>,
    transport: Option<Arc<dyn ForwardTransport>>,
) -> Vec<String> {
    wire.lock().unwrap().clear();
    let st = state(transport).await;
    let reg = json!({
        "id": "urn:ngsi-ld:ContextSourceRegistration:ft",
        "type": "ContextSourceRegistration",
        "mode": case.mode,
        "information": [{"entities": [{"type": "Vehicle"}]}],
        "operations": case.operations,
        "endpoint": format!("http://127.0.0.1:{port}"),
    });
    let (status, _, body) = send(&st, "POST", "csourceRegistrations", Some(reg)).await;
    assert_eq!(status, StatusCode::CREATED, "{}: {body}", case.caller);
    let (status, _, body) = send(&st, case.method, case.path, case.doc.clone()).await;
    assert!(status.as_u16() < 500, "{}: {status} {body}", case.caller);
    for _ in 0..200 {
        if !wire.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // what follows the first forward (a subscription's further copies) has
    // the same time to arrive in both runs
    tokio::time::sleep(Duration::from_millis(100)).await;
    wire.lock().unwrap().clone()
}

/// `raw` with every UUID masked: a distributed subscription's copy carries
/// an id each broker mints anew (5.8.1.4).
fn unminted(raw: &str) -> String {
    let shape = |w: &[u8]| {
        w.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
    };
    let bytes = raw.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes.len() - i >= 36 && shape(&bytes[i..i + 36]) {
            out.push_str("<uuid>");
            i += 36;
        } else {
            out.push(char::from(bytes[i]));
            i += 1;
        }
    }
    out
}

/// Whether `raw`, an HTTP/1.1 request as the peer read it, is `seen`.
fn is_on_the_wire(seen: &Seen, raw: &str, port: u16) -> bool {
    let Some((head, body)) = raw.split_once("\r\n\r\n") else {
        return false;
    };
    let mut lines = head.lines();
    let origin = format!("http://127.0.0.1:{port}");
    let line = format!(
        "{} {} HTTP/1.1",
        seen.method,
        seen.target.strip_prefix(&origin).unwrap_or(&seen.target)
    );
    lines.next() == Some(line.as_str())
        && body.as_bytes() == seen.body.as_slice()
        && seen.headers.iter().all(|(k, v)| {
            head.lines().any(|l| {
                l.split_once(": ")
                    .is_some_and(|(lk, lv)| lk.eq_ignore_ascii_case(k) && lv == v)
            })
        })
}

/// Each of the ten callers of `federation::forward` (ADR-0025's Cypher)
/// reaches the transport with the request the HTTP branch sends, and a
/// transport answering `None` changes no byte of what goes on the wire.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_forward_reaches_the_transport_and_none_keeps_http_byte_for_byte() {
    for case in cases() {
        let (port, wire) = peer();
        let plain = run(&case, port, &wire, None).await;
        let recorder = Arc::new(Recorder::default());
        let asked = run(&case, port, &wire, Some(recorder.clone())).await;
        let seen = recorder.0.lock().unwrap().clone();

        assert!(
            plain.first().is_some_and(|raw| raw.starts_with(case.line)),
            "{}: {plain:?}",
            case.caller
        );
        let masked = |all: &[String]| all.iter().map(|r| unminted(r)).collect::<Vec<_>>();
        assert_eq!(
            masked(&asked),
            masked(&plain),
            "{}: the transport changed the wire",
            case.caller
        );
        assert_eq!(seen.len(), asked.len(), "{}: {seen:?}", case.caller);
        for one in &seen {
            assert!(
                asked.iter().any(|raw| is_on_the_wire(one, raw, port)),
                "{}: the transport saw {one:?}, the wire carried {asked:?}",
                case.caller
            );
        }
    }
}

/// A transport that answers every forward with `answer` after `polls`
/// polls; `None` never answers.
struct Canned {
    answer: Option<ForwardAnswer>,
    polled: AtomicUsize,
}

impl ForwardTransport for Canned {
    fn forward<'a>(&'a self, _req: BuiltForward<'a>) -> Option<ForwardFuture<'a>> {
        let answer = self.answer.clone();
        Some(Box::pin(std::future::poll_fn(move |_| {
            self.polled.fetch_add(1, Ordering::SeqCst);
            match answer.clone() {
                Some(a) => std::task::Poll::Ready(a),
                None => std::task::Poll::Pending,
            }
        })))
    }
}

async fn query_through(
    transport: Arc<Canned>,
    management: Value,
) -> (StatusCode, Vec<String>, Value) {
    let st = state(Some(transport)).await;
    let reg = json!({
        "id": "urn:ngsi-ld:ContextSourceRegistration:canned",
        "type": "ContextSourceRegistration",
        "information": [{"entities": [{"type": "Vehicle"}]}],
        "operations": ["queryEntity"],
        "endpoint": "http://self.invalid",
        "management": management,
    });
    let (status, _, body) = send(&st, "POST", "csourceRegistrations", Some(reg)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    send(&st, "GET", "entities?type=Vehicle", None).await
}

/// The transport's answer is the forward's: its payload merges, and its
/// warnings travel on up to `MAX_PEER_WARNINGS`, as an HTTP peer's do.
#[tokio::test]
async fn the_transports_answer_is_the_forwards_with_its_warnings_capped() {
    let warnings: Vec<String> = (0..20).map(|i| format!("199 peer{i} \"w\"")).collect();
    let transport = Arc::new(Canned {
        answer: Some((
            200,
            json!([{"id": "urn:ngsi-ld:Vehicle:7", "type": "Vehicle"}]),
            warnings,
        )),
        polled: AtomicUsize::new(0),
    });

    let (status, got, body) = query_through(transport, json!({})).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body[0]["id"], "urn:ngsi-ld:Vehicle:7", "{body}");
    let relayed = got.iter().filter(|w| w.contains(" peer")).count();
    assert_eq!(relayed, antares_api::bounds::MAX_PEER_WARNINGS, "{got:?}");
}

/// 5.2.34 holds for a forward the transport serves: past `timeout` the
/// forward is a timeout, and inside the `cooldown` that follows the
/// transport's answer is not waited for at all.
#[tokio::test]
async fn timeout_and_cooldown_hold_for_the_transport() {
    let transport = Arc::new(Canned {
        answer: None,
        polled: AtomicUsize::new(0),
    });
    let management = json!({"timeout": 50, "cooldown": 60_000});
    let st = state(Some(transport.clone())).await;
    let reg = json!({
        "id": "urn:ngsi-ld:ContextSourceRegistration:slow",
        "type": "ContextSourceRegistration",
        "information": [{"entities": [{"type": "Vehicle"}]}],
        "operations": ["queryEntity"],
        "endpoint": "http://self.invalid",
        "management": management,
    });
    let (status, _, body) = send(&st, "POST", "csourceRegistrations", Some(reg)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let started = std::time::Instant::now();
    let (status, first, _) = send(&st, "GET", "entities?type=Vehicle", None).await;
    assert_eq!(status, StatusCode::OK);
    // ThreadSanitizer slows the whole request, not the 50 ms timeout: a
    // strict run took it past a flat 5 s (run 38109841097, T-3706)
    assert!(
        started.elapsed() < Duration::from_secs(5 * antares_api::state::slow_factor()),
        "the timeout held"
    );
    assert!(first.iter().any(|w| w.starts_with("199 ")), "{first:?}");
    let polled = transport.polled.load(Ordering::SeqCst);
    assert!(polled > 0);

    let (status, second, _) = send(&st, "GET", "entities?type=Vehicle", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(second.iter().any(|w| w.starts_with("199 ")), "{second:?}");
    assert_eq!(
        transport.polled.load(Ordering::SeqCst),
        polled,
        "in cooldown the source is not contacted"
    );
}

/// A second transport is refused: the choice is never registration order.
#[tokio::test]
async fn a_second_transport_is_refused() {
    let st = state(Some(Arc::new(Recorder::default()))).await;
    assert!(st
        .with_forward_transport(Arc::new(Recorder::default()))
        .is_err());
}
