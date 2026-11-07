// SPDX-License-Identifier: EUPL-1.2
//! ADR-0025 equivalence layer 1, "same request": for one forward, the
//! request the in-process transport hands the router
//! ([`antares_ops::in_process_request`]) equals the one the HTTP client
//! sends, in method, path, query, every header except `Host`,
//! `Content-Length` and the subject headers, and body bytes.
//!
//! A test transport records what the in-process builder makes of each
//! forward and declines it, so the very same `BuiltForward` goes on over
//! HTTP to a listener that records what arrives.

use antares_ops::test_kit::{allow_private_egress, TestBroker};
use antares_ops::{in_process_request, BuiltForward, ForwardFuture, ForwardTransport};
use axum::body::Body;
use axum::http::{HeaderMap, Request};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, PoisonError};
use tower::ServiceExt;

/// What one request carried, in the form both sides are compared in.
#[derive(Debug, PartialEq, Eq)]
struct Seen {
    method: String,
    path_and_query: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// The connection's, which the HTTP client derives from the socket and the
/// body. The subject headers are compared: neither side may carry them
/// (ADR-0020).
const NOT_COMPARED: [&str; 2] = ["host", "content-length"];

async fn seen(req: Request<Body>) -> Seen {
    let (parts, body) = req.into_parts();
    let body = body
        .collect()
        .await
        .map(|c| c.to_bytes().to_vec())
        .unwrap_or_default();
    Seen {
        method: parts.method.to_string(),
        path_and_query: parts
            .uri
            .path_and_query()
            .map(ToString::to_string)
            .unwrap_or_default(),
        headers: comparable(&parts.headers),
        body,
    }
}

fn comparable(headers: &HeaderMap) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = headers
        .iter()
        .filter(|(k, _)| !NOT_COMPARED.contains(&k.as_str()))
        .map(|(k, v)| {
            (
                k.as_str().to_owned(),
                String::from_utf8_lossy(v.as_bytes()).into_owned(),
            )
        })
        .collect();
    out.sort();
    out
}

type Held = Arc<Mutex<Vec<Request<Body>>>>;

/// Records the in-process request of every forward to `url`, then lets the
/// forward go over HTTP.
struct Capture {
    url: String,
    built: Held,
}

impl ForwardTransport for Capture {
    fn forward<'a>(&'a self, req: BuiltForward<'a>) -> Option<ForwardFuture<'a>> {
        if let Some(inner) = in_process_request(&req, &self.url) {
            self.built
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(inner);
        }
        None
    }
}

/// The context the `jsonldContext` cases recompact with, served by the
/// listener.
fn context() -> Value {
    json!({"@context": {"speed": "https://example.org/speed", "Car": "https://example.org/Car"}})
}

/// A listener recording every request it gets, answering as a context
/// source does; `/ctx.jsonld` serves [`context`].
async fn listener() -> (String, Held) {
    let got: Held = Arc::default();
    let kept = got.clone();
    let app = axum::Router::new().fallback(move |req: Request<Body>| {
        let kept = kept.clone();
        async move {
            let path = req.uri().path().to_owned();
            let method = req.method().clone();
            if path == "/ctx.jsonld" {
                return (
                    axum::http::StatusCode::OK,
                    [("content-type", "application/ld+json")],
                    context().to_string(),
                );
            }
            kept.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(req);
            match method.as_str() {
                "GET" => (
                    axum::http::StatusCode::OK,
                    [("content-type", "application/json")],
                    "[]".to_owned(),
                ),
                "POST" => (
                    axum::http::StatusCode::CREATED,
                    [("content-type", "application/json")],
                    String::new(),
                ),
                _ => (
                    axum::http::StatusCode::NO_CONTENT,
                    [("content-type", "application/json")],
                    String::new(),
                ),
            }
        }
    });
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|e| unreachable!("{e}"));
    let addr = socket.local_addr().unwrap_or_else(|e| unreachable!("{e}"));
    tokio::spawn(async move { axum::serve(socket, app).await });
    (format!("http://{addr}"), got)
}

/// `doc[key] = value` for an object `doc`.
fn set(doc: &mut Value, key: &str, value: Value) {
    if let Some(o) = doc.as_object_mut() {
        o.insert(key.to_owned(), value);
    }
}

/// One registration and the request whose forward is compared.
struct Case {
    name: &'static str,
    registration: Value,
    method: &'static str,
    path: &'static str,
    headers: Vec<(&'static str, &'static str)>,
    body: Option<Value>,
}

/// A Car; an `exclusive` or `redirect` registration names [`REMOTE`], which
/// is never created locally.
fn car(id: &str) -> Value {
    json!({"id": id, "type": "Car", "speed": {"type": "Property", "value": 3}})
}

const REMOTE: &str = "urn:ngsi-ld:Car:remote";

/// The Car an `inclusive` case creates, also locally: each case's own
/// (`-{n}` appended), so no create meets the one before it.
const LOCAL: &str = "urn:ngsi-ld:Car:local";

fn cases(url: &str) -> Vec<Case> {
    let reg = |mode: &str, extra: Value| {
        let mut doc = json!({
            "type": "ContextSourceRegistration",
            "endpoint": url,
            "mode": mode,
            "operations": [if mode == "auxiliary" { "retrieveOps" } else { "redirectionOps" }],
            "information": [{"entities": [{"type": "Car"}]}],
        });
        if mode != "inclusive" {
            let only =
                json!([{"entities": [{"id": REMOTE, "type": "Car"}], "propertyNames": ["speed"]}]);
            set(&mut doc, "information", only);
        }
        if let (Some(d), Some(e)) = (doc.as_object_mut(), extra.as_object()) {
            d.extend(e.clone());
        }
        doc
    };
    let query = "/ngsi-ld/v1/entities?type=Car&attrs=speed";
    let create = "/ngsi-ld/v1/entities";
    let mut out = Vec::new();
    for mode in ["inclusive", "exclusive", "redirect", "auxiliary"] {
        out.push(Case {
            name: "query",
            registration: reg(mode, json!({})),
            method: "GET",
            path: query,
            headers: Vec::new(),
            body: None,
        });
        out.push(Case {
            name: "query in a tenant, local only",
            registration: reg(
                mode,
                json!({"tenant": "b", "management": {"localOnly": true}}),
            ),
            method: "GET",
            path: query,
            headers: Vec::new(),
            body: None,
        });
        if mode != "auxiliary" {
            out.push(Case {
                name: "create",
                registration: reg(mode, json!({"tenant": "b"})),
                method: "POST",
                path: create,
                headers: Vec::new(),
                body: Some(car(if mode == "inclusive" { LOCAL } else { REMOTE })),
            });
        }
    }
    out.extend(options(url, &reg));
    out
}

/// The cases of the 4.3.6 options: `jsonldContext`, `contentType`, header
/// copies, an inline `@context` and an inbound `Via` chain.
fn options(url: &str, reg: &impl Fn(&str, Value) -> Value) -> Vec<Case> {
    let query = "/ngsi-ld/v1/entities?type=Car&attrs=speed";
    let create = "/ngsi-ld/v1/entities";
    let ctx = format!("{url}/ctx.jsonld");
    vec![
        Case {
            name: "jsonldContext",
            registration: reg(
                "inclusive",
                json!({"contextSourceInfo": [{"key": "jsonldContext", "value": ctx}]}),
            ),
            method: "POST",
            path: create,
            headers: Vec::new(),
            body: Some(car(LOCAL)),
        },
        Case {
            name: "contentType ld+json",
            registration: reg(
                "inclusive",
                json!({"contextSourceInfo": [{"key": "contentType", "value": "application/ld+json"}]}),
            ),
            method: "POST",
            path: create,
            headers: Vec::new(),
            body: Some(car(LOCAL)),
        },
        Case {
            name: "a header copied from the request, one fixed",
            registration: reg(
                "inclusive",
                json!({"contextSourceInfo": [
                    {"key": "X-Trace", "value": "urn:ngsi-ld:request"},
                    {"key": "X-Fixed", "value": "f"},
                    {"key": "X-Subject", "value": "urn:ngsi-ld:request"}]}),
            ),
            method: "GET",
            path: query,
            headers: vec![("X-Trace", "t-1")],
            body: None,
        },
        Case {
            name: "an inline @context",
            registration: reg("inclusive", json!({})),
            method: "POST",
            path: create,
            headers: vec![("Content-Type", "application/ld+json")],
            body: Some({
                let mut c = car("urn:ngsi-ld:Car:inline");
                let ctx = json!([{"speed": "https://example.org/speed"},
                                 "https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.8.jsonld"]);
                set(&mut c, "@context", ctx);
                c
            }),
        },
        Case {
            name: "an inbound Via chain",
            registration: reg("inclusive", json!({})),
            method: "GET",
            path: query,
            headers: vec![("Via", "1.1 upstream-a, 1.1 upstream-b")],
            body: None,
        },
    ]
}

/// `method path` in Tenant `a` as subject `alice`; the status and body.
async fn request(
    app: &axum::Router,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&Value>,
) -> (u16, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("NGSILD-Tenant", "a")
        .header("X-Subject", "alice");
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let req = match body {
        Some(b) => {
            let text = b.to_string();
            if !headers.iter().any(|(k, _)| *k == "Content-Type") {
                req = req.header("Content-Type", "application/json");
            }
            req.header("Content-Length", text.len())
                .body(Body::from(text))
        }
        None => req.body(Body::empty()),
    }
    .unwrap_or_else(|e| unreachable!("{e}"));
    let answer = app
        .clone()
        .oneshot(req)
        .await
        .unwrap_or_else(|e| unreachable!("{e}"));
    let status = answer.status().as_u16();
    let bytes = answer
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn drained(held: &Held) -> Vec<Request<Body>> {
    std::mem::take(&mut *held.lock().unwrap_or_else(PoisonError::into_inner))
}

/// Layer 1: per mode, with and without `tenant` and `localOnly`, with
/// `jsonldContext`, `contentType`, `urn:ngsi-ld:request` copies, an inline
/// `@context` and an inbound `Via` chain, both builders make one request.
#[tokio::test]
async fn icsr_builds_the_http_request() {
    std::env::set_var("ANTARES_POLICY_SUBJECT_HEADERS", "X-Subject");
    allow_private_egress();
    let (url, arrived) = listener().await;
    let built: Held = Arc::default();
    let capture = Capture {
        url: url.clone(),
        built: built.clone(),
    };
    let broker = TestBroker::new()
        .await
        .and_then(|b| b.with_forward_transport(move |_| Arc::new(capture)))
        .unwrap_or_else(|e| unreachable!("{e}"));
    let app = broker.router();
    let mut compared = 0_usize;
    for (n, case) in cases(&url).iter().enumerate() {
        let id = format!("urn:ngsi-ld:ContextSourceRegistration:r{n}");
        let mut reg = case.registration.clone();
        set(&mut reg, "id", json!(id));
        let registrations = "/ngsi-ld/v1/csourceRegistrations";
        let (made, why) = request(&app, "POST", registrations, &[], Some(&reg)).await;
        assert_eq!(made, 201, "{}: register {why}", case.name);
        drained(&built);
        drained(&arrived);
        let body = case.body.clone().map(|mut b| {
            if b.get("id").and_then(Value::as_str) == Some(LOCAL) {
                set(&mut b, "id", json!(format!("{LOCAL}-{n}")));
            }
            b
        });
        let body = body.as_ref();
        let (status, why) = request(&app, case.method, case.path, &case.headers, body).await;
        assert!(status < 500, "{}: {status} {why}", case.name);
        let built = drained(&built);
        let arrived = drained(&arrived);
        let counts = (built.len(), arrived.len());
        assert_eq!(counts, (1, 1), "{}: forwards built and sent", case.name);
        for (b, a) in built.into_iter().zip(arrived) {
            let (b, a) = (seen(b).await, seen(a).await);
            let subject = b.headers.iter().any(|(k, _)| k == "x-subject");
            assert!(!subject, "{}", case.name);
            assert_eq!(b, a, "{}", case.name);
            compared = compared.saturating_add(1);
        }
        let path = format!("{registrations}/{id}");
        let (gone, why) = request(&app, "DELETE", &path, &[], None).await;
        assert_eq!(gone, 204, "{}: unregister {why}", case.name);
    }
    assert_eq!(compared, cases(&url).len());
}
