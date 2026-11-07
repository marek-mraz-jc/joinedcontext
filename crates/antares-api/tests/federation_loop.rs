// SPDX-License-Identifier: EUPL-1.2
//! Loop detection over the Via header (6.3.17 / 6.3.18, Tables 5.2.9-1 and
//! 5.2.40-1), end to end through the router against a mock Context Source.
//!
//! The scenario is ETSI `D018_01`: register a source, create through it, read
//! the Via pseudonym the broker sent, then replay it on a DELETE. What the
//! spec makes of that replay depends on the registration:
//!
//! * a single **exclusive/redirect** source looping back is the one case
//!   6.3.17 p.278 answers with **508 Loop Detected**;
//! * an **inclusive** source is dropped from matching instead (Table 6.3.18-2)
//!   and the operation runs locally — 508 would fail an operation the broker
//!   can serve;
//! * the same chain in **another tenant** is not a loop at all: Table 5.2.40-1
//!   makes the alias tenant-specific, so `antares1` and `antares1~zvolen` are
//!   different Context Sources.

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tower::ServiceExt;

const ALIAS: &str = "antares1";
const ENTITY: &str = "urn:ngsi-ld:Vehicle:d018";

/// A canned Context Source: replies `reply` to every request, counts hits,
/// and keeps the head and body of the last request it saw.
struct Mock {
    port: u16,
    hits: Arc<AtomicUsize>,
    last_head: Arc<std::sync::Mutex<String>>,
    last_body: Arc<std::sync::Mutex<String>>,
}

fn mock_replying(reply: &'static str) -> Mock {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    // The mock serves ONE request per connection, so it must say so: without
    // Connection: close the broker's pooled client may reuse the (dropped)
    // keep-alive connection for the next forward — a CI-timing flake where a
    // per-entity fallback forward dies on the dead socket and `hits` undercounts.
    let reply = reply.replacen("\r\n", "\r\nConnection: close\r\n", 1);
    let hits: Arc<AtomicUsize> = Arc::default();
    let last_head: Arc<std::sync::Mutex<String>> = Arc::default();
    let last_body: Arc<std::sync::Mutex<String>> = Arc::default();
    let (seen, head, seen_body) = (hits.clone(), last_head.clone(), last_body.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            seen.fetch_add(1, Ordering::SeqCst);
            // One `read` returns whatever has arrived, which is not the same
            // as the request: a head split across TCP segments would be
            // recorded truncated, and a body larger than the buffer would
            // still be in flight when the reply closed the socket. Read to
            // the blank line, then to the length the head declares.
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                match s.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
            }
            let head_end = buf
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .map(|p| p + 4)
                .unwrap_or(buf.len());
            let headers = String::from_utf8_lossy(&buf[..head_end]).to_string();
            let want: usize = headers
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse().unwrap_or(0))
                })
                .unwrap_or(0);
            while buf.len() - head_end < want {
                match s.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
            }
            *head.lock().expect("lock") = headers.trim_end().to_owned();
            *seen_body.lock().expect("lock") =
                String::from_utf8_lossy(&buf[head_end..]).to_string();
            let _ = s.write_all(reply.as_bytes());
        }
    });
    Mock {
        port,
        hits,
        last_head,
        last_body,
    }
}

/// A Context Source that answers every request `204` and counts the hits.
fn mock_source() -> (u16, Arc<AtomicUsize>) {
    let m = mock_replying("HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n");
    (m.port, m.hits)
}

/// A Context Source that accepts, reads, and never answers — the 6.3.17
/// "fails to respond in time" source. The socket is held open so the
/// client's total timeout (8 s), not a connection reset, ends the wait.
fn mock_stalling() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 8192];
            let _ = s.read(&mut buf);
            held.push(s); // keep the connection open, never reply
        }
    });
    port
}

async fn send(st: &AppState, req: Request<Body>) -> axum::http::Response<Body> {
    antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response")
}

/// Register one Context Source for `ENTITY` in `tenant`.
async fn register(st: &AppState, tenant: Option<&str>, mode: &str, port: u16, alias: Option<&str>) {
    let mut doc = serde_json::json!({
        "id": format!("urn:ngsi-ld:ContextSourceRegistration:{mode}-{}", alias.unwrap_or("none")),
        "type": "ContextSourceRegistration",
        "mode": mode,
        // 5.9.2.4: auxiliary registrations are limited to retrieve/query ops
        "operations": if mode == "auxiliary" { ["retrieveOps"] } else { ["redirectionOps"] },
        "information": [{"entities": [{"type": "Vehicle", "id": ENTITY}]}],
        "endpoint": format!("http://127.0.0.1:{port}"),
    });
    if let Some(a) = alias {
        doc["contextSourceAlias"] = serde_json::json!(a);
    }
    let body = doc.to_string();
    let mut req = Request::builder()
        .method("POST")
        .uri("/ngsi-ld/v1/csourceRegistrations")
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len());
    if let Some(t) = tenant {
        req = req.header("NGSILD-Tenant", t);
    }
    let res = send(st, req.body(Body::from(body)).expect("request")).await;
    assert_eq!(res.status(), StatusCode::CREATED, "registration create");
}

fn delete_with_via(tenant: Option<&str>, via: &str) -> Request<Body> {
    let mut req = Request::builder()
        .method("DELETE")
        .uri(format!("/ngsi-ld/v1/entities/{ENTITY}"))
        .header("Via", via);
    if let Some(t) = tenant {
        req = req.header("NGSILD-Tenant", t);
    }
    req.body(Body::empty()).expect("request")
}

fn state() -> AppState {
    // The mock source is loopback, denied by the egress policy by default.
    // Set once: a sibling test reading the variable while another rewrites
    // it saw the policy missing and refused the forward.
    antares_jsonld::allow_private_egress(true);
    AppState::new(ALIAS.into())
}

/// 6.3.17 p.278: "In the case of an exclusive or redirect registration, where
/// all of the data is held outside of the Context Broker and held in a single
/// registered source … 508 Loop Detected — if the single registered source
/// and tenant is registered to redirect back on to the Context Broker."
#[tokio::test(flavor = "multi_thread")]
async fn single_redirect_source_looping_back_is_508() {
    let st = state();
    let (port, _) = mock_source();
    register(&st, None, "redirect", port, None).await;

    let res = send(&st, delete_with_via(None, "1.1 antares1")).await;
    assert_eq!(res.status(), StatusCode::LOOP_DETECTED);
}

/// TS 104 175 9.7: "If the requested endpoint is also listed as previously
/// encountered when a Context Source receives a call to that endpoint, this is
/// an indication that a new loop has been detected. A suitable error shall be
/// returned". Owner decision (2026-10-10, T-3459): a write whose `Via` names
/// this broker is answered 508 whatever the registration's mode, and nothing
/// is forwarded; a read with the same chain is served from the local store
/// and says so in a 199 warning (6.3.17).
#[tokio::test(flavor = "multi_thread")]
async fn clause_9_7_self_in_via_with_inclusive_registration_is_a_detected_loop() {
    let st = state();
    let body = serde_json::json!({"id": ENTITY, "type": "Vehicle"}).to_string();
    let res = send(
        &st,
        Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/entities")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request"),
    )
    .await;
    assert_eq!(res.status(), StatusCode::CREATED, "local entity");
    let (port, hits) = mock_source();
    register(&st, None, "inclusive", port, None).await;

    let res = send(&st, delete_with_via(None, "1.1 antares1")).await;
    assert_eq!(res.status(), StatusCode::LOOP_DETECTED);
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "the looping registration must not be forwarded to"
    );

    let res = send(
        &st,
        Request::builder()
            .uri(format!("/ngsi-ld/v1/entities/{ENTITY}"))
            .header("Via", "1.1 antares1")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK, "a read is served locally");
    let warning = res
        .headers()
        .get("NGSILD-Warning")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert!(warning.starts_with("199 "), "the read says so: {warning:?}");
    assert_eq!(hits.load(Ordering::SeqCst), 0, "nor is the read forwarded");
}

/// The same rule with no registration at all: the call came back to this
/// broker, which is the loop 9.7 describes.
#[tokio::test(flavor = "multi_thread")]
async fn clause_9_7_self_in_via_without_a_registration_is_a_loop_on_writes() {
    let st = state();
    let res = send(&st, delete_with_via(None, "1.1 antares1")).await;
    assert_eq!(res.status(), StatusCode::LOOP_DETECTED);
}

/// Table 5.2.40-1: "In the multi-tenancy use case … this id shall be
/// identifying a specific Tenant within a registered Context Source." A chain
/// naming this broker's DEFAULT tenant says nothing about tenant `zvolen`, so
/// federation there proceeds — one alias for the whole process turned every
/// cross-tenant federation inside one broker into a phantom loop.
#[tokio::test(flavor = "multi_thread")]
async fn a_chain_naming_another_tenant_is_not_a_loop() {
    let st = state();
    let (port, hits) = mock_source();
    register(&st, Some("zvolen"), "redirect", port, None).await;

    let res = send(&st, delete_with_via(Some("zvolen"), "1.1 antares1")).await;
    assert_ne!(
        res.status(),
        StatusCode::LOOP_DETECTED,
        "antares1 (default tenant) is not antares1~zvolen"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1, "the forward must happen");

    // …and the tenant-qualified pseudonym IS detected as the loop it is
    let res = send(&st, delete_with_via(Some("zvolen"), "1.1 antares1~zvolen")).await;
    assert_eq!(res.status(), StatusCode::LOOP_DETECTED);
    assert_eq!(hits.load(Ordering::SeqCst), 1, "no second forward");
}

/// Table 6.3.18-2 + 5.2.9: a registration carrying the `contextSourceAlias`
/// of a source already in the chain is not a matching registration — the
/// request has been there. Nothing is forwarded, and it is not this broker's
/// own loop, so no 508 either.
#[tokio::test(flavor = "multi_thread")]
async fn a_registered_source_already_in_the_chain_is_skipped() {
    let st = state();
    let (port, hits) = mock_source();
    register(&st, None, "redirect", port, Some("peer1")).await;

    let res = send(&st, delete_with_via(None, "1.1 peer1")).await;
    assert_ne!(res.status(), StatusCode::LOOP_DETECTED);
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "a source already in the Via chain must not be contacted again"
    );
}

/// RFC 7230 section 3.2.2: Via is a list header — the chain may be split across any
/// number of `Via:` field lines and the forms are equivalent. A pseudonym in
/// the SECOND field is still a loop; reading only the first field would run
/// the cycle undetected.
#[tokio::test(flavor = "multi_thread")]
async fn via_split_across_header_fields_still_detects_the_loop() {
    let st = state();
    let (port, _) = mock_source();
    register(&st, None, "redirect", port, None).await;

    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/ngsi-ld/v1/entities/{ENTITY}"))
        .header("Via", "1.1 upstream")
        .header("Via", "1.1 antares1") // second FIELD, same header
        .body(Body::empty())
        .expect("request");
    let res = send(&st, req).await;
    assert_eq!(res.status(), StatusCode::LOOP_DETECTED);
}

/// The outbound chain preserves EVERY inbound Via field before appending our
/// own pseudonym — truncating it would delete the hop history downstream
/// brokers use for their own loop detection (6.3.18 Table 6.3.18-2).
#[tokio::test(flavor = "multi_thread")]
async fn forwarding_preserves_all_inbound_via_fields() {
    let st = state();
    let m = mock_replying("HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n");
    register(&st, None, "inclusive", m.port, None).await;

    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/ngsi-ld/v1/entities/{ENTITY}"))
        .header("Via", "1.1 up1")
        .header("Via", "1.1 up2")
        .body(Body::empty())
        .expect("request");
    send(&st, req).await;

    assert_eq!(m.hits.load(Ordering::SeqCst), 1, "forward must happen");
    let head = m.last_head.lock().expect("lock").clone();
    assert!(
        head.contains("Via: 1.1 up1, 1.1 up2, 1.1 antares1"),
        "both inbound fields + our pseudonym, in order — got:\n{head}"
    );
}

/// Malformed Via elements must neither panic nor false-positive, and an
/// element carrying an RFC 7230 comment — `1.1 antares1 (proxy)` — is still
/// our pseudonym (received-by is the token before the comment).
#[tokio::test(flavor = "multi_thread")]
async fn malformed_via_elements_are_tolerated() {
    let st = state();
    let (port, hits) = mock_source();
    register(&st, None, "redirect", port, None).await;

    // junk chain, nothing matches → not a loop, forward proceeds
    let res = send(&st, delete_with_via(None, ", ,   1.1, ;;; (x)")).await;
    assert_ne!(res.status(), StatusCode::LOOP_DETECTED);
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    // comment after the pseudonym is not part of the token
    let res = send(&st, delete_with_via(None, "1.1 antares1 (proxy)")).await;
    assert_eq!(res.status(), StatusCode::LOOP_DETECTED);
}

/// 6.3.18 Table 6.3.18-2 makes the outbound chain the inbound chain plus
/// this broker, and a federation that spans many brokers arrives with the
/// hops of all of them. A long chain is where the two halves of loop
/// detection can quietly disagree: the parser has to read to the END of the
/// list to find a pseudonym that is not near its front, and the forward has
/// to carry the whole thing on so the next broker can do the same. A chain
/// that outgrows one read of the socket is the case that shows both.
#[tokio::test(flavor = "multi_thread")]
async fn a_via_chain_longer_than_one_read_is_parsed_and_forwarded_whole() {
    let st = state();
    let m = mock_replying("HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n");
    register(&st, None, "redirect", m.port, None).await;

    // Long in bytes, not in hops: 30 elements stay under MAX_VIA_HOPS, and
    // a Via pseudonym is a token, so the length lives in the names.
    let hops: Vec<String> = (0..30)
        .map(|i| format!("1.1 hop{i:02}{}", "x".repeat(300)))
        .collect();
    let chain = hops.join(", ");
    assert!(chain.len() > 8 * 1024, "the chain spans more than one read");

    let res = send(&st, delete_with_via(None, &chain)).await;
    assert_ne!(
        res.status(),
        StatusCode::LOOP_DETECTED,
        "no hop in the chain is this broker"
    );
    assert_eq!(m.hits.load(Ordering::SeqCst), 1, "the forward must happen");

    let head = m.last_head.lock().expect("lock").clone();
    assert!(
        head.contains("hop00") && head.contains("hop29"),
        "the forwarded chain was cut: {} bytes",
        head.len()
    );
    assert!(
        head.contains(ALIAS),
        "this broker appends its own pseudonym to what it passes on"
    );

    // The same chain with our pseudonym at its very END is still a loop:
    // stopping the scan early is the failure this length is here to catch.
    let looped = format!("{chain}, 1.1 {ALIAS}");
    let res = send(&st, delete_with_via(None, &looped)).await;
    assert_eq!(res.status(), StatusCode::LOOP_DETECTED, "{}", looped.len());
    assert_eq!(m.hits.load(Ordering::SeqCst), 1, "no second forward");
}

/// 6.3.17 p.278, single proxied source: "404 Not Found — if resources not
/// found within the single registered source." The source's 404 IS the
/// operation's 404.
#[tokio::test(flavor = "multi_thread")]
async fn single_proxy_source_404_passes_through() {
    let st = state();
    let m = mock_replying(
        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
    );
    register(&st, None, "redirect", m.port, None).await;

    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/ngsi-ld/v1/entities/{ENTITY}"))
        .body(Body::empty())
        .expect("request");
    let res = send(&st, req).await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

/// 6.3.17 p.278, single proxied source: "502 Bad Gateway — if the single
/// forwarded request fails for any other reason such as the Context Broker
/// itself having insufficient access rights." A peer's 500 (or 401/403) is
/// not this operation's status — it surfaces as 502.
#[tokio::test(flavor = "multi_thread")]
async fn single_proxy_source_failure_surfaces_as_502() {
    let st = state();
    let m = mock_replying("HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n");
    register(&st, None, "redirect", m.port, None).await;

    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/ngsi-ld/v1/entities/{ENTITY}"))
        .body(Body::empty())
        .expect("request");
    let res = send(&st, req).await;
    assert_eq!(res.status(), StatusCode::BAD_GATEWAY);
}

/// 6.3.17 p.278, single proxied source: "504 Gateway Timeout — if the single
/// registered source fails to respond in time." The source accepts and goes
/// silent; the broker's own forward deadline (8 s at construction) must
/// end the wait. Slowest test in the file — it IS the timeout.
#[tokio::test(flavor = "multi_thread")]
async fn single_proxy_source_timeout_is_504() {
    let st = state();
    let port = mock_stalling();
    register(&st, None, "redirect", port, None).await;

    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/ngsi-ld/v1/entities/{ENTITY}"))
        .body(Body::empty())
        .expect("request");
    let res = send(&st, req).await;
    assert_eq!(res.status(), StatusCode::GATEWAY_TIMEOUT);
}

/// 6.3.17 p.278, inclusive registration: "when updating the state of the
/// distributed entity, an error response is returned from one or more
/// registered sources: 207 Multi Status." Local success + remote failure is
/// a partial result, never a rollback and never a plain error.
#[tokio::test(flavor = "multi_thread")]
async fn inclusive_partial_failure_is_207() {
    let st = state();
    // entity exists locally BEFORE any registration → created purely locally
    let body = serde_json::json!({"id": ENTITY, "type": "Vehicle"}).to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/ngsi-ld/v1/entities")
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .expect("request");
    assert_eq!(send(&st, req).await.status(), StatusCode::CREATED);

    let m = mock_replying("HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n");
    register(&st, None, "inclusive", m.port, None).await;

    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/ngsi-ld/v1/entities/{ENTITY}"))
        .body(Body::empty())
        .expect("request");
    let res = send(&st, req).await;
    assert_eq!(res.status(), StatusCode::MULTI_STATUS);
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .expect("body");
    let doc: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    // 5.2.16 success: "Array of Entity IDs corresponding to the Elements
    // that were successfully treated"; 5.2.17 BatchEntityError entityId
    // "Entity ID corresponding to the Entity in error" and registrationId
    // "Registration Id corresponding to a failed distributed operation".
    assert_eq!(doc["success"], serde_json::json!([ENTITY]), "{doc}");
    assert_eq!(doc["errors"][0]["entityId"], ENTITY, "{doc}");
    assert_eq!(
        doc["errors"][0]["registrationId"], "urn:ngsi-ld:ContextSourceRegistration:inclusive-none",
        "{doc}"
    );
    assert_eq!(
        doc["errors"][0]["error"]["status"], 500,
        "an inclusive source's error keeps its own status in the 207: {doc}"
    );
}

/// POST one Context Source Registration document.
async fn register_doc(st: &AppState, doc: serde_json::Value) {
    let body = doc.to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/ngsi-ld/v1/csourceRegistrations")
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .expect("request");
    assert_eq!(
        send(st, req).await.status(),
        StatusCode::CREATED,
        "registration create"
    );
}

/// A registration of `mode` for ENTITY claiming `attrs` at `port`.
fn csr(id: &str, mode: &str, ops: &[&str], attrs: &[&str], port: u16) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "type": "ContextSourceRegistration",
        "mode": mode,
        "operations": ops,
        "information": [{"entities": [{"type": "Vehicle", "id": ENTITY}], "propertyNames": attrs}],
        "endpoint": format!("http://127.0.0.1:{port}"),
    })
}

async fn post_json(
    st: &AppState,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let body = body.to_string();
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .expect("request");
    let res = send(st, req).await;
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

fn vehicle(attrs: &[&str]) -> serde_json::Value {
    let mut doc = serde_json::json!({"id": ENTITY, "type": "Vehicle"});
    for a in attrs {
        doc[*a] = serde_json::json!({"type": "Property", "value": 1});
    }
    doc
}

const VOCAB: &str = "https://uri.etsi.org/ngsi-ld/default-context/";

/// TS 104 176 7.2.3.2 / CIM 009 6.4.3.1: a distributed Create Entity with
/// one part failing answers 207 with a BatchOperationResult. 5.2.16 success
/// holds Entity IDs; 5.2.17 BatchEntityError: entityId "Entity ID
/// corresponding to the Entity in error", registrationId "Registration Id
/// corresponding to a failed distributed operation". Coraine
/// csource-reg-distops-create-207 / -409.
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_2_17_distributed_create_207_is_a_batch_operation_result() {
    let st = state();
    let ok = mock_replying("HTTP/1.1 201 Created\r\nContent-Length: 0\r\n\r\n");
    let down = mock_replying("HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n");
    let ops = ["createEntity", "retrieveEntity"];
    register_doc(
        &st,
        csr("urn:CSR:OK", "redirect", &ops, &["speed"], ok.port),
    )
    .await;
    register_doc(
        &st,
        csr("urn:CSR:Down", "redirect", &ops, &["color"], down.port),
    )
    .await;
    let (status, doc) = post_json(&st, "/ngsi-ld/v1/entities", vehicle(&["speed", "color"])).await;
    assert_eq!(status, StatusCode::MULTI_STATUS, "{doc}");
    assert_eq!(doc["success"], serde_json::json!([ENTITY]), "{doc}");
    let errors = doc["errors"].as_array().expect("errors");
    assert_eq!(errors.len(), 1, "{doc}");
    assert_eq!(errors[0]["entityId"], ENTITY, "{doc}");
    assert_eq!(errors[0]["registrationId"], "urn:CSR:Down", "{doc}");
    // inside the 207 each error keeps the source's own status
    assert_eq!(errors[0]["error"]["status"], 503, "{doc}");
}

/// 5.6.1.4: "For matching Registrations where the Create Entity operation
/// is not supported, this shall result in an error of type Conflict if the
/// complete Create Entity operation failed or in a partial success if some
/// parts of it succeeded." Coraine csource-reg-distops-create-excl-no-op.
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_6_1_4_unsupported_part_is_a_conflict_inside_the_207() {
    let st = state();
    let m = mock_source();
    let reg = csr(
        "urn:CSR:NoCreate",
        "exclusive",
        &["retrieveEntity"],
        &["speed"],
        m.0,
    );
    register_doc(&st, reg).await;
    let (status, doc) = post_json(&st, "/ngsi-ld/v1/entities", vehicle(&["speed", "color"])).await;
    assert_eq!(status, StatusCode::MULTI_STATUS, "{doc}");
    assert_eq!(doc["success"], serde_json::json!([ENTITY]), "{doc}");
    assert_eq!(doc["errors"][0]["entityId"], ENTITY, "{doc}");
    assert_eq!(
        doc["errors"][0]["registrationId"], "urn:CSR:NoCreate",
        "{doc}"
    );
    assert_eq!(doc["errors"][0]["error"]["status"], 409, "{doc}");
    assert_eq!(
        doc["errors"][0]["error"]["type"], "https://uri.etsi.org/ngsi-ld/errors/Conflict",
        "{doc}"
    );
    assert_eq!(
        m.1.load(Ordering::SeqCst),
        0,
        "the source is never contacted"
    );
}

/// 5.6.3.4: "If an exclusive or redirect Context Source Registration
/// matches against the input data, the Attributes from matching input data
/// are forwarded for remote processing." An Attribute the registration does
/// not claim never reaches the source. Coraine
/// csource-reg-distops-append-exclusive.
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_6_3_4_append_forwards_only_the_registered_attributes() {
    let st = state();
    let (status, _) = post_json(&st, "/ngsi-ld/v1/entities", vehicle(&[])).await;
    assert_eq!(status, StatusCode::CREATED);
    let m = mock_replying("HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n");
    let ops = ["appendAttrs", "retrieveEntity"];
    register_doc(
        &st,
        csr("urn:CSR:AppendExcl", "exclusive", &ops, &["speed"], m.port),
    )
    .await;
    let fragment = serde_json::json!({
        "speed": {"type": "Property", "value": 99},
        "brand": {"type": "Property", "value": "Zephyr"},
    });
    let uri = format!("/ngsi-ld/v1/entities/{ENTITY}/attrs");
    let (status, doc) = post_json(&st, &uri, fragment).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{doc}");
    let sent: serde_json::Value =
        serde_json::from_str(&m.last_body.lock().expect("lock")).expect("forwarded json");
    assert_eq!(
        sent["speed"]["value"], 99,
        "the claimed Attribute is forwarded: {sent}"
    );
    assert!(
        sent.get("brand").is_none(),
        "brand is not the source's: {sent}"
    );
}

/// 5.6.2.4: an Update Attributes fragment names its Entity in the path; an
/// `id` member inside the fragment names nothing. The ETSI payload
/// vehicle-speed-different-attribute.jsonld carries
/// `"id": "urn:ngsi-ld:Vehicle:randomUUID"`, and D004_01_exc expects the
/// exclusive source to receive the PATCH all the same.
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_6_2_4_a_fragment_id_does_not_unmatch_the_registration() {
    let st = state();
    let (status, _) = post_json(&st, "/ngsi-ld/v1/entities", vehicle(&[])).await;
    assert_eq!(status, StatusCode::CREATED);
    let m = mock_replying("HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n");
    let ops = ["updateEntity", "retrieveEntity"];
    register_doc(
        &st,
        csr("urn:CSR:UpdExcl", "exclusive", &ops, &["speed"], m.port),
    )
    .await;
    let body = serde_json::json!({
        "id": "urn:ngsi-ld:Vehicle:randomUUID",
        "type": "Vehicle",
        "speed": {"type": "Property", "value": 66},
    })
    .to_string();
    let req = Request::builder()
        .method("PATCH")
        .uri(format!("/ngsi-ld/v1/entities/{ENTITY}/attrs/"))
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .expect("request");
    let res = send(&st, req).await;
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .expect("body");
    let doc = String::from_utf8_lossy(&bytes);
    assert_eq!(status, StatusCode::NO_CONTENT, "{doc}");
    assert_eq!(
        m.hits.load(Ordering::SeqCst),
        1,
        "the source gets the PATCH"
    );
    let sent: serde_json::Value =
        serde_json::from_str(&m.last_body.lock().expect("lock")).expect("forwarded json");
    assert_eq!(sent["speed"]["value"], 66, "{sent}");
    assert_eq!(
        sent.get("id"),
        None,
        "the placeholder id stays here: {sent}"
    );
}

/// 5.2.18 UpdateResult updated: "List of Attributes (represented by their
/// name) that were appended or updated." A source answering its own 207
/// UpdateResult is unfolded: what it updated is updated, what it refused is
/// notUpdated with its reason; an Attribute it reports outside its
/// registration does not count (4.3.6.1). Coraine
/// csource-reg-distops-updated-dedup.
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_2_18_a_source_update_result_is_unfolded() {
    let st = state();
    let speed = mock_replying(
        "HTTP/1.1 207 Multi-Status\r\nContent-Type: application/json\r\nContent-Length: 37\r\n\r\n{\"updated\":[\"speed\"],\"notUpdated\":[]}",
    );
    let color = mock_replying(
        "HTTP/1.1 207 Multi-Status\r\nContent-Type: application/json\r\nContent-Length: 101\r\n\r\n{\"updated\":[\"color\",\"speed\"],\"notUpdated\":[{\"attributeName\":\"paint\",\"reason\":\"read-only at source\"}]}",
    );
    let ops = ["appendAttrs"];
    register_doc(
        &st,
        csr("urn:CSR:Speed", "redirect", &ops, &["speed"], speed.port),
    )
    .await;
    register_doc(
        &st,
        csr(
            "urn:CSR:Color",
            "redirect",
            &ops,
            &["color", "paint"],
            color.port,
        ),
    )
    .await;
    let uri = format!("/ngsi-ld/v1/entities/{ENTITY}/attrs");
    let mut frag = vehicle(&["speed", "color", "paint"]);
    if let Some(o) = frag.as_object_mut() {
        o.remove("id");
        o.remove("type");
    }
    let (status, doc) = post_json(&st, &uri, frag).await;
    assert_eq!(status, StatusCode::MULTI_STATUS, "{doc}");
    let iri = |a: &str| format!("{VOCAB}{a}");
    // updated is a list of names, order not defined: each one once
    let mut updated: Vec<&str> = doc["updated"]
        .as_array()
        .expect("updated")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    updated.sort_unstable();
    assert_eq!(updated, [iri("color"), iri("speed")], "{doc}");
    assert_eq!(
        doc["notUpdated"],
        serde_json::json!([{
            "attributeName": iri("paint"),
            "reason": "read-only at source",
            "registrationId": "urn:CSR:Color",
        }]),
        "{doc}"
    );
}

async fn get_json(st: &AppState, uri: &str) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .expect("request");
    let res = send(st, req).await;
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

/// 6.3.17 p.278 (TS 104 176 6.3.5): "In the case of an exclusive or redirect
/// registration, where all of the data is held outside of the Context Broker
/// and held in a single registered source, the following errors shall be
/// returned: ... 504 Gateway Timeout - if the single registered source fails
/// to respond in time ... 502 Bad Gateway - if the single forwarded request
/// fails for any other reason". Retrieve Entity is no exception. Coraine
/// csource-reg-distops-retrieve-upstream-error.
#[tokio::test(flavor = "multi_thread")]
async fn clause_6_3_17_single_source_read_failure_is_502() {
    for (reply, want) in [
        (
            "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n",
            StatusCode::BAD_GATEWAY,
        ),
        (
            "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n",
            StatusCode::BAD_GATEWAY,
        ),
        (
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n",
            StatusCode::NOT_FOUND,
        ),
    ] {
        let st = state();
        let m = mock_replying(reply);
        register_doc(
            &st,
            csr(
                "urn:CSR:Src",
                "exclusive",
                &["retrieveEntity"],
                &["speed"],
                m.port,
            ),
        )
        .await;
        let (status, doc) = get_json(&st, &format!("/ngsi-ld/v1/entities/{ENTITY}")).await;
        assert_eq!(status, want, "{reply}: {doc}");
        if want == StatusCode::BAD_GATEWAY {
            assert_eq!(doc["status"], 502, "{doc}");
            assert_eq!(doc["registrationId"], "urn:CSR:Src", "{doc}");
        }
    }
}

/// 6.3.17 p.278: "504 Gateway Timeout - if the single registered source
/// fails to respond in time", for a redirect and an exclusive registration;
/// a refused connection is the 502. Coraine distop_retrieve_timeout_504.
#[tokio::test(flavor = "multi_thread")]
async fn clause_6_3_17_single_source_read_timeout_is_504() {
    let port = mock_stalling();
    for mode in ["redirect", "exclusive"] {
        let st = state();
        let mut reg = csr("urn:CSR:Slow", mode, &["retrieveEntity"], &["speed"], port);
        reg["management"] = serde_json::json!({"timeout": 200});
        register_doc(&st, reg).await;
        let (status, doc) = get_json(&st, &format!("/ngsi-ld/v1/entities/{ENTITY}")).await;
        assert_eq!(status, StatusCode::GATEWAY_TIMEOUT, "{mode}: {doc}");
        assert_eq!(doc["registrationId"], "urn:CSR:Slow", "{doc}");
    }
    // bound, never listening: every connection is refused
    let (_held, closed) = crate::common::net::refused_port();
    let st = state();
    let mut reg = csr(
        "urn:CSR:Dead",
        "exclusive",
        &["retrieveEntity"],
        &["speed"],
        closed,
    );
    reg["management"] = serde_json::json!({"timeout": 200});
    register_doc(&st, reg).await;
    let (status, doc) = get_json(&st, &format!("/ngsi-ld/v1/entities/{ENTITY}")).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{doc}");
}

/// 4.5.23.1 (TS 104 175 7.7.1): "Only Relationships targeting a locally
/// stored Entity or Relationships annotated with an objectType whose object
/// is an Internal Linked Entity are considered to be retrievable in this
/// manner." A target a registration holds under the Relationship's
/// objectType joins; one without objectType, or whose objectType no
/// registration holds it under, does not, and is never asked for. Coraine
/// linked_entities_objecttype_gate.
#[tokio::test(flavor = "multi_thread")]
async fn clause_4_5_23_1_object_typed_link_joins_a_registered_entity() {
    let st = state();
    let m = mock_replying(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 73\r\n\r\n{\"id\":\"urn:E2\",\"type\":\"T2\",\"P\":{\"type\":\"Property\",\"value\":\"from-CP1-E2\"}}",
    );
    register_doc(
        &st,
        serde_json::json!({
            "id": "urn:CSR:remote",
            "type": "ContextSourceRegistration",
            "mode": "inclusive",
            "information": [{"entities": [
                {"id": "urn:E2", "type": "T2"},
                {"id": "urn:E3", "type": "T3"},
                {"id": "urn:E5", "type": "T5"},
            ]}],
            "endpoint": format!("http://127.0.0.1:{}", m.port),
        }),
    )
    .await;
    let local = serde_json::json!({"id": "urn:E4", "type": "T4", "P": {"type": "Property", "value": "local-E4"}});
    assert_eq!(
        post_json(&st, "/ngsi-ld/v1/entities", local).await.0,
        StatusCode::CREATED
    );
    let linking = serde_json::json!({
        "id": "urn:E1",
        "type": "T1",
        "linkLocalNoType": {"type": "Relationship", "object": "urn:E4"},
        "linkRemoteTyped": {"type": "Relationship", "object": "urn:E2", "objectType": "T2"},
        "linkRemoteWrongType": {"type": "Relationship", "object": "urn:E5", "objectType": "T9"},
        "linkRemoteUntyped": {"type": "Relationship", "object": "urn:E3"},
    });
    assert_eq!(
        post_json(&st, "/ngsi-ld/v1/entities", linking).await.0,
        StatusCode::CREATED
    );
    let (status, doc) = get_json(&st, "/ngsi-ld/v1/entities/urn:E1?join=inline&joinLevel=1").await;
    assert_eq!(status, StatusCode::OK, "{doc}");
    assert_eq!(
        doc["linkLocalNoType"]["entity"]["P"]["value"], "local-E4",
        "{doc}"
    );
    assert_eq!(
        doc["linkRemoteTyped"]["entity"]["P"]["value"], "from-CP1-E2",
        "{doc}"
    );
    assert!(doc["linkRemoteWrongType"].get("entity").is_none(), "{doc}");
    assert!(doc["linkRemoteUntyped"].get("entity").is_none(), "{doc}");
    assert_eq!(m.hits.load(Ordering::SeqCst), 1, "only urn:E2 is asked for");
    // local=true: no registration is considered (6.3.18)
    let (_, doc) = get_json(
        &st,
        "/ngsi-ld/v1/entities/urn:E1?join=inline&joinLevel=1&local=true",
    )
    .await;
    assert!(doc["linkRemoteTyped"].get("entity").is_none(), "{doc}");
}

/// 4.3.6.2 p.41 + 6.3.17 p.278: "In the case of an auxiliary registration
/// HTTP unsafe methods are not supported" — auxiliary distributed operations
/// are limited to consumption (5.7). Writes never reach an auxiliary source.
#[tokio::test(flavor = "multi_thread")]
async fn auxiliary_source_never_receives_unsafe_methods() {
    let st = state();
    let (port, hits) = mock_source();
    register(&st, None, "auxiliary", port, None).await;

    let body = serde_json::json!({"id": ENTITY, "type": "Vehicle"}).to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/ngsi-ld/v1/entities")
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .expect("request");
    assert_eq!(send(&st, req).await.status(), StatusCode::CREATED);

    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/ngsi-ld/v1/entities/{ENTITY}"))
        .body(Body::empty())
        .expect("request");
    assert_eq!(send(&st, req).await.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "create and delete are unsafe methods — the auxiliary source must see neither"
    );
}

/// 4.3.6.2 p.41: an auxiliary source "never overrides data held directly
/// within a Context Broker … only included if it is supplementary". Same
/// attribute → local wins; new attribute → supplemented.
#[tokio::test(flavor = "multi_thread")]
async fn auxiliary_data_never_overrides_local() {
    let st = state();
    let body = serde_json::json!({
        "id": ENTITY, "type": "Vehicle",
        "brandName": {"type": "Property", "value": "local-value"},
    })
    .to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/ngsi-ld/v1/entities")
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .expect("request");
    assert_eq!(send(&st, req).await.status(), StatusCode::CREATED);

    let remote = serde_json::json!({
        "id": ENTITY, "type": "Vehicle",
        "brandName": {"type": "Property", "value": "aux-value"},
        "color": {"type": "Property", "value": "blue"},
    })
    .to_string();
    let reply: &'static str = Box::leak(
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{remote}",
            remote.len()
        )
        .into_boxed_str(),
    );
    let m = mock_replying(reply);
    register(&st, None, "auxiliary", m.port, None).await;

    let req = Request::builder()
        .method("GET")
        .uri(format!("/ngsi-ld/v1/entities/{ENTITY}"))
        .header("Accept", "application/json")
        .body(Body::empty())
        .expect("request");
    let res = send(&st, req).await;
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .expect("body");
    let doc: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(
        doc["brandName"]["value"], "local-value",
        "auxiliary data must not override the broker's own: {doc}"
    );
    assert_eq!(
        doc["color"]["value"], "blue",
        "supplementary auxiliary data must be included: {doc}"
    );
}

/// Table 6.3.18-1: "If local=true then no Context Source Registrations shall
/// be considered as matching" — and Table 6.4.3.2-1 makes `type=*` imply
/// local=true. Either way the registered source is never contacted.
#[tokio::test(flavor = "multi_thread")]
async fn local_true_and_type_wildcard_suppress_forwarding() {
    let st = state();
    let (port, hits) = mock_source();
    register(&st, None, "inclusive", port, None).await;

    for uri in [
        "/ngsi-ld/v1/entities?type=Vehicle&local=true".to_owned(),
        "/ngsi-ld/v1/entities?type=*".to_owned(),
        format!("/ngsi-ld/v1/entities/{ENTITY}?local=true"),
    ] {
        let req = Request::builder()
            .method("GET")
            .uri(&uri)
            .header("Accept", "application/json")
            .body(Body::empty())
            .expect("request");
        let res = send(&st, req).await;
        assert_ne!(res.status(), StatusCode::BAD_GATEWAY, "{uri}");
        assert_eq!(
            hits.load(Ordering::SeqCst),
            0,
            "{uri}: local scope must not contact the registered source"
        );
    }
}

/// Table 6.3.17-1: a registration loop on a distributed GET is abnormal
/// behaviour — "199 Miscellaneous Warning: … a registration loop has been
/// detected" — surfaced as NGSILD-Warning, with the response still served
/// from local data.
#[tokio::test(flavor = "multi_thread")]
async fn suppressed_loop_forward_warns_199_on_get() {
    let st = state();
    let body = serde_json::json!({"id": ENTITY, "type": "Vehicle"}).to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/ngsi-ld/v1/entities")
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .expect("request");
    assert_eq!(send(&st, req).await.status(), StatusCode::CREATED);

    let (port, hits) = mock_source();
    register(&st, None, "inclusive", port, None).await;

    let req = Request::builder()
        .method("GET")
        .uri(format!("/ngsi-ld/v1/entities/{ENTITY}"))
        .header("Accept", "application/json")
        .header("Via", "1.1 antares1")
        .body(Body::empty())
        .expect("request");
    let res = send(&st, req).await;
    assert_eq!(res.status(), StatusCode::OK, "local data still serves");
    let warning = res
        .headers()
        .get("NGSILD-Warning")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert!(
        warning.starts_with("199 antares1 "),
        "Table 6.3.17-1 warn form `199 <alias> \"…\"`, got {warning:?}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0, "the forward was suppressed");
}

/// 5.6.1.4: a redirect (or exclusive) registration matching the input but
/// NOT supporting Create Entity yields an error of type Conflict and is
/// never contacted; an inclusive registration without Create Entity support
/// is not forwarded — the local create proceeds.
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_6_1_create_unsupported_by_registration() {
    let post_entity = || {
        let body = serde_json::json!({"id": ENTITY, "type": "Vehicle",
                "speed": {"type": "Property", "value": 1}})
        .to_string();
        Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/entities")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request")
    };
    let register_ops = |st: AppState, mode: &'static str, port: u16| async move {
        let doc = serde_json::json!({
            "id": format!("urn:ngsi-ld:ContextSourceRegistration:ops-{mode}"),
            "type": "ContextSourceRegistration",
            "mode": mode,
            "operations": ["retrieveEntity"],
            "information": [{"entities": [{"type": "Vehicle", "id": ENTITY}]}],
            "endpoint": format!("http://127.0.0.1:{port}"),
        });
        let body = doc.to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/csourceRegistrations")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request");
        assert_eq!(send(&st, req).await.status(), StatusCode::CREATED);
        st
    };

    // redirect without createEntity: the complete create fails → Conflict,
    // and the registered source is never contacted
    let st = register_ops(state(), "redirect", {
        let (port, hits) = mock_source();
        REDIRECT_HITS.set(hits).ok();
        port
    })
    .await;
    let res = send(&st, post_entity()).await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
    let body = String::from_utf8_lossy(
        &axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .expect("body"),
    )
    .into_owned();
    assert!(
        body.contains("errors/Conflict"),
        "error type must be Conflict: {body}"
    );
    assert_eq!(
        REDIRECT_HITS.get().expect("hits").load(Ordering::SeqCst),
        0,
        "unsupported operation must not be forwarded"
    );

    // inclusive without createEntity: not forwarded, local create succeeds
    let (port, hits) = mock_source();
    let st = register_ops(state(), "inclusive", port).await;
    let res = send(&st, post_entity()).await;
    assert_eq!(res.status(), StatusCode::CREATED, "local create proceeds");
    assert_eq!(hits.load(Ordering::SeqCst), 0, "no forward without support");
}

static REDIRECT_HITS: std::sync::OnceLock<Arc<AtomicUsize>> = std::sync::OnceLock::new();

/// 5.6.2.4: an exclusive/redirect registration matching the update but NOT
/// supporting it yields Conflict (never contacted); an inclusive one without
/// support is not forwarded and the local update proceeds.
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_6_2_update_unsupported_by_registration() {
    let patch_attrs = || {
        let body = serde_json::json!({"speed": {"type": "Property", "value": 9}}).to_string();
        Request::builder()
            .method("PATCH")
            .uri(format!("/ngsi-ld/v1/entities/{ENTITY}/attrs"))
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request")
    };
    let register_retrieve_only = |st: AppState, mode: &'static str, port: u16| async move {
        let doc = serde_json::json!({
            "id": format!("urn:ngsi-ld:ContextSourceRegistration:upd-{mode}"),
            "type": "ContextSourceRegistration",
            "mode": mode,
            "operations": ["retrieveOps"],
            "information": [{"entities": [{"type": "Vehicle", "id": ENTITY}]}],
            "endpoint": format!("http://127.0.0.1:{port}"),
        });
        let body = doc.to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/csourceRegistrations")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request");
        assert_eq!(send(&st, req).await.status(), StatusCode::CREATED);
        st
    };

    // redirect without update support → Conflict, source untouched
    let (port, hits) = mock_source();
    let st = register_retrieve_only(state(), "redirect", port).await;
    let res = send(&st, patch_attrs()).await;
    let status = res.status();
    let body = String::from_utf8_lossy(
        &axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .expect("body"),
    )
    .into_owned();
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "complete update failed: {body}"
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "unsupported op never forwarded"
    );

    // inclusive without update support → local update proceeds, no forward
    let (port, hits) = mock_source();
    let st = register_retrieve_only(state(), "inclusive", port).await;
    let body = serde_json::json!({"id": ENTITY, "type": "Vehicle",
        "speed": {"type": "Property", "value": 1}})
    .to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/ngsi-ld/v1/entities?local=true")
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .expect("request");
    assert_eq!(send(&st, req).await.status(), StatusCode::CREATED);
    let res = send(&st, patch_attrs()).await;
    assert_eq!(
        res.status(),
        StatusCode::NO_CONTENT,
        "local update proceeds"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0, "no forward without support");
}

/// 5.6.7.4: per matching CSR — batch op supported → forward the batch;
/// only the single Create Entity op supported → forward PER-ENTITY create
/// requests; neither supported on a proxy mode → Conflict; the source is
/// never contacted with an operation it does not support.
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_6_7_batch_create_forwarding_fallbacks() {
    let batch_create = || {
        let body = serde_json::json!([
            {"id": "urn:ngsi-ld:Vehicle:b1", "type": "Vehicle",
             "speed": {"type": "Property", "value": 1}},
            {"id": "urn:ngsi-ld:Vehicle:b2", "type": "Vehicle",
             "speed": {"type": "Property", "value": 2}}
        ])
        .to_string();
        Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/entityOperations/create")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request")
    };
    let register_with = |st: AppState, ops: serde_json::Value, port: u16| async move {
        let doc = serde_json::json!({
            "id": "urn:ngsi-ld:ContextSourceRegistration:batch-fb",
            "type": "ContextSourceRegistration",
            "mode": "redirect",
            "operations": ops,
            "information": [{"entities": [{"type": "Vehicle"}]}],
            "endpoint": format!("http://127.0.0.1:{port}"),
        });
        let body = doc.to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/csourceRegistrations")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request");
        assert_eq!(send(&st, req).await.status(), StatusCode::CREATED);
        st
    };

    // createEntity-only source: the batch falls back to per-entity creates
    let m = mock_replying("HTTP/1.1 201 Created\r\nContent-Length: 0\r\n\r\n");
    let st = register_with(state(), serde_json::json!(["createEntity"]), m.port).await;
    let res = send(&st, batch_create()).await;
    // TS 104 176 7.7.3.1: every entity created, so 201 with the id array
    assert_eq!(
        res.status(),
        StatusCode::CREATED,
        "fallback forwarding must not fail the batch"
    );
    assert_eq!(
        m.hits.load(Ordering::SeqCst),
        2,
        "one Create Entity forward per entity"
    );
    // 5.6.1 Create Entity posts to the COLLECTION: an id segment would be
    // the Entity resource (6.5), which defines no POST — so the request line
    // is asserted whole, not by prefix.
    assert!(
        m.last_head
            .lock()
            .expect("lock")
            .starts_with("POST /ngsi-ld/v1/entities HTTP/1.1"),
        "fallback posts to the entity collection: {}",
        m.last_head.lock().expect("lock")
    );

    // retrieve-only proxy source: Conflict, never contacted
    let (port, hits) = mock_source();
    let st = register_with(state(), serde_json::json!(["retrieveOps"]), port).await;
    let res = send(&st, batch_create()).await;
    let body = String::from_utf8_lossy(
        &axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .expect("body"),
    )
    .into_owned();
    assert!(
        body.contains("does not accept") || body.contains("Conflict"),
        "unsupported proxy source reports Conflict: {body}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0, "never contacted");
}

/// 5.6.8.4 support ladder: no upsertBatch → per-entity Create Entity, and on
/// AlreadyExists fall back to Replace Entity (mode replace/unset) or Update
/// Attributes (mode update); a source offering only updateEntity gets the
/// per-entity PATCH directly under options=update.
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_6_8_upsert_forwarding_fallbacks() {
    let upsert = |q: &str| {
        let body = serde_json::json!([
            {"id": ENTITY, "type": "Vehicle", "speed": {"type": "Property", "value": 1}}
        ])
        .to_string();
        Request::builder()
            .method("POST")
            .uri(format!("/ngsi-ld/v1/entityOperations/upsert{q}"))
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request")
    };
    let register_with = |st: AppState, ops: serde_json::Value, port: u16| async move {
        let doc = serde_json::json!({
            "id": "urn:ngsi-ld:ContextSourceRegistration:ups-fb",
            "type": "ContextSourceRegistration",
            "mode": "redirect",
            "operations": ops,
            "information": [{"entities": [{"type": "Vehicle", "id": ENTITY}]}],
            "endpoint": format!("http://127.0.0.1:{port}"),
        });
        let body = doc.to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/csourceRegistrations")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request");
        assert_eq!(send(&st, req).await.status(), StatusCode::CREATED);
        st
    };

    // createEntity+replaceEntity source, remote already has the entity
    // (mock answers 409): create → 409 → Replace Entity forward
    let m =
        mock_replying("HTTP/1.1 409 Conflict\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
    let st = register_with(
        state(),
        serde_json::json!(["createEntity", "replaceEntity"]),
        m.port,
    )
    .await;
    let _ = send(&st, upsert("")).await;
    assert_eq!(
        m.hits.load(Ordering::SeqCst),
        2,
        "create then replace fallback"
    );
    assert!(
        m.last_head
            .lock()
            .expect("lock")
            .starts_with(&format!("PUT /ngsi-ld/v1/entities/{ENTITY}")),
        "AlreadyExists falls back to Replace Entity: {}",
        m.last_head.lock().expect("lock")
    );

    // updateEntity-only source with options=update: direct per-entity PATCH
    let m = mock_replying("HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n");
    let st = register_with(state(), serde_json::json!(["updateEntity"]), m.port).await;
    let _ = send(&st, upsert("?options=update")).await;
    assert_eq!(m.hits.load(Ordering::SeqCst), 1, "one update forward");
    assert!(
        m.last_head
            .lock()
            .expect("lock")
            .starts_with(&format!("PATCH /ngsi-ld/v1/entities/{ENTITY}/attrs")),
        "update mode uses Update Attributes: {}",
        m.last_head.lock().expect("lock")
    );
}

/// 5.6.8.5: a 201 Upsert response lists ONLY the newly created ids — a
/// forwarded upsertBatch answered with 204 means those entities were
/// UPDATED on the source, so an all-updated distributed upsert returns
/// 204 (matrix-9 D013_01/02: every remote batch success was tagged
/// "created" and the aggregate flipped to 201).
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_6_8_remote_204_upsert_is_update_not_create() {
    let m =
        mock_replying("HTTP/1.1 204 No Content\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
    let st = state();
    let reg = serde_json::json!({
        "id": "urn:ngsi-ld:ContextSourceRegistration:ups-204",
        "type": "ContextSourceRegistration",
        "mode": "redirect",
        "operations": ["upsertBatch"],
        "information": [{"entities": [{"type": "Vehicle", "id": ENTITY}]}],
        "endpoint": format!("http://127.0.0.1:{}", m.port),
    })
    .to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/ngsi-ld/v1/csourceRegistrations")
        .header("Content-Type", "application/json")
        .header("Content-Length", reg.len())
        .body(Body::from(reg))
        .expect("request");
    assert_eq!(send(&st, req).await.status(), StatusCode::CREATED);

    let body = serde_json::json!([
        {"id": ENTITY, "type": "Vehicle", "speed": {"type": "Property", "value": 7}}
    ])
    .to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/ngsi-ld/v1/entityOperations/upsert")
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .expect("request");
    let res = send(&st, req).await;
    assert_eq!(res.status(), StatusCode::NO_CONTENT, "all updated → 204");
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    assert!(
        bytes.is_empty(),
        "204 carries no created-id list: {}",
        String::from_utf8_lossy(&bytes)
    );
    assert_eq!(m.hits.load(Ordering::SeqCst), 1, "one batch forward");
}

/// 5.6.8.4: S holds "one [Entity ID] for each NGSI-LD Entity which was
/// successfully processed". Under an inclusive registration the Entity is
/// created locally AND on the source, and its id is listed once (Coraine
/// entity_batch_upsert_distops_inclusive listed it twice).
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_6_8_inclusive_upsert_lists_each_created_id_once() {
    let body = format!("[\"{ENTITY}\"]");
    let reply = format!(
        "HTTP/1.1 201 Created\r\nConnection: close\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let m = mock_replying(Box::leak(reply.into_boxed_str()));
    let st = state();
    let reg = serde_json::json!({
        "id": "urn:ngsi-ld:ContextSourceRegistration:ups-incl",
        "type": "ContextSourceRegistration",
        "mode": "inclusive",
        "operations": ["upsertBatch"],
        "information": [{"entities": [{"type": "Vehicle", "id": ENTITY}]}],
        "endpoint": format!("http://127.0.0.1:{}", m.port),
    })
    .to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/ngsi-ld/v1/csourceRegistrations")
        .header("Content-Type", "application/json")
        .header("Content-Length", reg.len())
        .body(Body::from(reg))
        .expect("request");
    assert_eq!(send(&st, req).await.status(), StatusCode::CREATED);
    let body = serde_json::json!([
        {"id": ENTITY, "type": "Vehicle", "speed": {"type": "Property", "value": 7}}
    ])
    .to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/ngsi-ld/v1/entityOperations/upsert")
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .expect("request");
    let res = send(&st, req).await;
    assert_eq!(res.status(), StatusCode::CREATED);
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    let ids: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(ids, serde_json::json!([ENTITY]));
    assert_eq!(m.hits.load(Ordering::SeqCst), 1, "one batch forward");
}

/// The other half of that rule. 5.6.8.4's distributed branch says to "Merge
/// the returned list of Entities successfully created with S", so an Entity
/// a Context Source created is one this request created — and 5.6.8.5's
/// second bullet then applies to the whole answer: 201, S carrying that id.
/// The local copy exists first, so the local half replaces rather than
/// creates and the 201 can only have come from the source.
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_6_8_a_remote_201_puts_its_created_ids_in_s() {
    // the id spelled out, because the mock serves one static reply
    let m = mock_replying(
        "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\n\
         Content-Length: 28\r\n\r\n[\"urn:ngsi-ld:Vehicle:d018\"]",
    );
    let st = state();
    let reg = serde_json::json!({
        "id": "urn:ngsi-ld:ContextSourceRegistration:ups-201",
        "type": "ContextSourceRegistration",
        "mode": "inclusive",
        "operations": ["upsertBatch"],
        "information": [{"entities": [{"type": "Vehicle", "id": ENTITY}]}],
        "endpoint": format!("http://127.0.0.1:{}", m.port),
    })
    .to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/ngsi-ld/v1/csourceRegistrations")
        .header("Content-Type", "application/json")
        .header("Content-Length", reg.len())
        .body(Body::from(reg))
        .expect("request");
    assert_eq!(send(&st, req).await.status(), StatusCode::CREATED);

    // the local copy, so the local half of the upsert updates
    let seed = serde_json::json!({"id": ENTITY, "type": "Vehicle",
                                  "speed": {"type": "Property", "value": 1}})
    .to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/ngsi-ld/v1/entities")
        .header("Content-Type", "application/json")
        .header("Content-Length", seed.len())
        .body(Body::from(seed))
        .expect("request");
    assert_eq!(send(&st, req).await.status(), StatusCode::CREATED);

    let body = serde_json::json!([
        {"id": ENTITY, "type": "Vehicle", "speed": {"type": "Property", "value": 7}}
    ])
    .to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/ngsi-ld/v1/entityOperations/upsert")
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .expect("request");
    let res = send(&st, req).await;
    assert_eq!(res.status(), StatusCode::CREATED, "the source created one");
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    let doc: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(
        doc,
        serde_json::json!([ENTITY]),
        "S is the created ids alone (5.6.8.5)"
    );
}

/// 5.6.21.4: a matched registration forwards the purge only when it
/// supports purgeEntity; an unsupported matched registration — any mode,
/// redirect included — contributes a Conflict and is never contacted.
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_6_21_purge_forwarding_gate() {
    let purge_req = || {
        Request::builder()
            .method("DELETE")
            .uri("/ngsi-ld/v1/entities?type=Vehicle")
            .body(Body::empty())
            .expect("request")
    };
    let register_with = |st: AppState, ops: serde_json::Value, port: u16| async move {
        let doc = serde_json::json!({
            "id": "urn:ngsi-ld:ContextSourceRegistration:purge-gate",
            "type": "ContextSourceRegistration",
            "mode": "redirect",
            "operations": ops,
            "information": [{"entities": [{"type": "Vehicle", "id": ENTITY}]}],
            "endpoint": format!("http://127.0.0.1:{port}"),
        });
        let body = doc.to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/csourceRegistrations")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request");
        assert_eq!(send(&st, req).await.status(), StatusCode::CREATED);
        st
    };

    // redirect source without purgeEntity: Conflict, never contacted
    let (port, hits) = mock_source();
    let st = register_with(state(), serde_json::json!(["retrieveOps"]), port).await;
    let res = send(&st, purge_req()).await;
    let body = String::from_utf8_lossy(
        &axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .expect("body"),
    )
    .into_owned();
    assert!(
        body.contains("does not accept") || body.contains("Conflict"),
        "unsupported matched source reports Conflict: {body}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0, "never contacted");

    // purgeEntity-supporting source: one DELETE /entities forward with the
    // narrowing query on the wire
    let m = mock_replying("HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n");
    let st = register_with(state(), serde_json::json!(["purgeEntity"]), m.port).await;
    let _ = send(&st, purge_req()).await;
    assert_eq!(m.hits.load(Ordering::SeqCst), 1, "one purge forward");
    let head = m.last_head.lock().expect("lock").clone();
    assert!(
        head.starts_with("DELETE /ngsi-ld/v1/entities?") && head.contains("type=Vehicle"),
        "forward keeps the purge query: {head}"
    );
}

/// 5.7.3.4: matching registrations that support retrieveTemporal are
/// forwarded GET /temporal/entities/{id} and the remote instance data
/// merges into the Temporal Evolution; sources without the operation are
/// never contacted.
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_7_3_temporal_retrieve_forwarding() {
    let register_with = |st: AppState, ops: serde_json::Value, port: u16| async move {
        let doc = serde_json::json!({
            "id": "urn:ngsi-ld:ContextSourceRegistration:trf",
            "type": "ContextSourceRegistration",
            "mode": "redirect",
            "operations": ops,
            "information": [{"entities": [{"type": "Vehicle", "id": ENTITY}]}],
            "endpoint": format!("http://127.0.0.1:{port}"),
        });
        let body = doc.to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/csourceRegistrations")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request");
        assert_eq!(send(&st, req).await.status(), StatusCode::CREATED);
        st
    };
    let temporal_get = || {
        Request::builder()
            .uri(format!("/ngsi-ld/v1/temporal/entities/{ENTITY}"))
            .body(Body::empty())
            .expect("request")
    };

    // retrieveTemporal-supporting source: forwarded, remote data served
    let remote = serde_json::json!({
        "id": ENTITY, "type": "Vehicle",
        "speed": [{"type": "Property", "value": 5,
                   "observedAt": "2026-01-01T00:00:00Z"}]
    })
    .to_string();
    let reply: &'static str = Box::leak(
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            remote.len(),
            remote
        )
        .into_boxed_str(),
    );
    let m = mock_replying(reply);
    let st = register_with(state(), serde_json::json!(["retrieveTemporal"]), m.port).await;
    let res = send(&st, temporal_get()).await;
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .expect("body");
    let doc: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    let speed = doc["speed"].as_array().expect("speed instances");
    assert_eq!(speed.len(), 1, "remote instance served: {doc}");
    assert_eq!(speed[0]["value"], 5);
    assert_eq!(m.hits.load(Ordering::SeqCst), 1, "one temporal forward");
    assert!(
        m.last_head
            .lock()
            .expect("lock")
            .starts_with(&format!("GET /ngsi-ld/v1/temporal/entities/{ENTITY}")),
        "forward targets the temporal resource: {}",
        m.last_head.lock().expect("lock")
    );

    // source without retrieveTemporal: never contacted; nothing local → 404
    let (port, hits) = mock_source();
    let st = register_with(state(), serde_json::json!(["retrieveEntity"]), port).await;
    let res = send(&st, temporal_get()).await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(hits.load(Ordering::SeqCst), 0, "never contacted");
}

/// 5.7.4.4: the temporal query forwards to matching registrations that
/// support queryTemporal (with the temporal window on the wire); sources
/// without the operation are never contacted.
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_7_4_temporal_query_forwarding() {
    let register_with = |st: AppState, ops: serde_json::Value, port: u16| async move {
        let doc = serde_json::json!({
            "id": "urn:ngsi-ld:ContextSourceRegistration:tqf",
            "type": "ContextSourceRegistration",
            "mode": "redirect",
            "operations": ops,
            "information": [{"entities": [{"type": "Vehicle", "id": ENTITY}]}],
            "endpoint": format!("http://127.0.0.1:{port}"),
        });
        let body = doc.to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/csourceRegistrations")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request");
        assert_eq!(send(&st, req).await.status(), StatusCode::CREATED);
        st
    };
    let temporal_query = || {
        Request::builder()
            .uri(
                "/ngsi-ld/v1/temporal/entities?type=Vehicle&timerel=after\
                 &timeAt=2020-01-01T00:00:00Z",
            )
            .body(Body::empty())
            .expect("request")
    };

    let remote = serde_json::json!([{
        "id": ENTITY, "type": "Vehicle",
        "speed": [{"type": "Property", "value": 7,
                   "observedAt": "2026-01-01T00:00:00Z"}]
    }])
    .to_string();
    let reply: &'static str = Box::leak(
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            remote.len(),
            remote
        )
        .into_boxed_str(),
    );
    let m = mock_replying(reply);
    let st = register_with(state(), serde_json::json!(["queryTemporal"]), m.port).await;
    let res = send(&st, temporal_query()).await;
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .expect("body");
    let docs: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    let arr = docs.as_array().expect("array");
    assert_eq!(arr.len(), 1, "remote temporal entity served: {docs}");
    assert_eq!(arr[0]["speed"][0]["value"], 7, "{docs}");
    assert_eq!(
        m.hits.load(Ordering::SeqCst),
        1,
        "one temporal query forward"
    );
    let head = m.last_head.lock().expect("lock").clone();
    assert!(
        head.starts_with("GET /ngsi-ld/v1/temporal/entities?") && head.contains("timerel=after"),
        "forward carries the temporal window: {head}"
    );

    // retrieveTemporal-only source: not eligible for the query fan-out
    let (port, hits) = mock_source();
    let st = register_with(state(), serde_json::json!(["retrieveTemporal"]), port).await;
    let res = send(&st, temporal_query()).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(hits.load(Ordering::SeqCst), 0, "never contacted");
}

/// 4.3.6.3 (TS 104 175 9.3.3): with an exclusive registration the "Context
/// Broker itself is not permitted to hold context data about the registered
/// Entities and Attributes locally (thus all registered context data is
/// obtained from the external registered sources)". The local history of the
/// claimed Attribute stays out of a temporal query and a temporal retrieve;
/// the unclaimed one stays in. Coraine
/// troe_timescale_temporal_distop_query_exclusive.
#[tokio::test(flavor = "multi_thread")]
async fn clause_4_3_6_3_exclusive_temporal_reads_drop_the_local_history() {
    let local = serde_json::json!({
        "id": ENTITY, "type": "Vehicle",
        "speed": [{"type": "Property", "value": 10, "observedAt": "2026-04-28T09:00:00Z"}],
        "fuel": [{"type": "Property", "value": 90, "observedAt": "2026-04-28T09:00:00Z"}],
    });
    let remote = serde_json::json!({
        "id": ENTITY, "type": "Vehicle",
        "speed": [{"type": "Property", "value": 30, "observedAt": "2026-04-28T11:00:00Z"}],
    });
    let reply = |body: serde_json::Value| -> &'static str {
        let body = body.to_string();
        Box::leak(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .into_boxed_str(),
        )
    };
    let values = |doc: &serde_json::Value, attr: &str| -> Vec<i64> {
        doc[attr]
            .as_array()
            .map(|a| a.iter().filter_map(|i| i["value"].as_i64()).collect())
            .unwrap_or_default()
    };
    for (path, answer) in [
        (
            "/ngsi-ld/v1/temporal/entities?type=Vehicle&timerel=after&timeAt=2026-04-28T00:00:00Z"
                .to_owned(),
            serde_json::json!([remote]),
        ),
        (
            format!(
                "/ngsi-ld/v1/temporal/entities/{ENTITY}?timerel=after&timeAt=2026-04-28T00:00:00Z"
            ),
            remote.clone(),
        ),
    ] {
        let st = state();
        let (status, doc) = post_json(&st, "/ngsi-ld/v1/temporal/entities", local.clone()).await;
        assert_eq!(status, StatusCode::CREATED, "{doc}");
        let m = mock_replying(reply(answer));
        let ops = ["queryTemporal", "retrieveTemporal"];
        register_doc(
            &st,
            csr("urn:CSR:TExcl", "exclusive", &ops, &["speed"], m.port),
        )
        .await;
        let (status, doc) = get_json(&st, &path).await;
        assert_eq!(status, StatusCode::OK, "{path}: {doc}");
        let doc = if doc.is_array() { doc[0].clone() } else { doc };
        assert_eq!(
            values(&doc, "speed"),
            [30],
            "{path}: only the source's speed: {doc}"
        );
        assert_eq!(
            values(&doc, "fuel"),
            [90],
            "{path}: unclaimed fuel stays: {doc}"
        );
    }
}

/// 5.6.20.4 support ladder: no mergeBatch → per-entity Merge Entity
/// (PATCH /entities/{id}) forwards.
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_6_20_batch_merge_fallback() {
    let m = mock_replying("HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n");
    let st = state();
    let doc = serde_json::json!({
        "id": "urn:ngsi-ld:ContextSourceRegistration:bm-fb",
        "type": "ContextSourceRegistration",
        "mode": "redirect",
        "operations": ["mergeEntity"],
        "information": [{"entities": [{"type": "Vehicle", "id": ENTITY}]}],
        "endpoint": format!("http://127.0.0.1:{}", m.port),
    });
    let body = doc.to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/ngsi-ld/v1/csourceRegistrations")
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .expect("request");
    assert_eq!(send(&st, req).await.status(), StatusCode::CREATED);
    let body = serde_json::json!([
        {"id": ENTITY, "type": "Vehicle", "speed": {"type": "Property", "value": 3}}
    ])
    .to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/ngsi-ld/v1/entityOperations/merge")
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .expect("request");
    let _ = send(&st, req).await;
    assert_eq!(m.hits.load(Ordering::SeqCst), 1, "one Merge Entity forward");
    assert!(
        m.last_head
            .lock()
            .expect("lock")
            .starts_with(&format!("PATCH /ngsi-ld/v1/entities/{ENTITY} ")),
        "fallback uses the single-entity merge: {}",
        m.last_head.lock().expect("lock")
    );
}

/// 5.6.20.4 local behaviour: each entity merged per 5.6.17 (RFC 7396 —
/// value overwrite, null deletes the attribute); 204 with no body when all
/// merge, 207 with the S/E partition when some fail (5.6.20.5).
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_6_20_local_merge_and_partition() {
    let st = state();
    let post = |uri: &'static str, body: String| {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request")
    };
    let entity = serde_json::json!({
        "id": ENTITY, "type": "Vehicle",
        "speed": {"type": "Property", "value": 3},
        "color": {"type": "Property", "value": "red"}
    })
    .to_string();
    assert_eq!(
        send(&st, post("/ngsi-ld/v1/entities", entity))
            .await
            .status(),
        StatusCode::CREATED
    );

    // all merged → 204, no body; overwrite + null-deletion + append applied
    let batch = serde_json::json!([{
        "id": ENTITY, "type": "Vehicle",
        "speed": {"type": "Property", "value": 9},
        "color": "urn:ngsi-ld:null",
        "brand": {"type": "Property", "value": "Ajax"}
    }])
    .to_string();
    let res = send(&st, post("/ngsi-ld/v1/entityOperations/merge", batch)).await;
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .expect("body");
    assert!(bytes.is_empty(), "204 carries no body: {bytes:?}");
    let req = Request::builder()
        .uri(format!("/ngsi-ld/v1/entities/{ENTITY}"))
        .body(Body::empty())
        .expect("request");
    let res = send(&st, req).await;
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .expect("body");
    let doc: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(doc["speed"]["value"], 9, "value overwritten: {doc}");
    assert_eq!(doc["brand"]["value"], "Ajax", "attribute appended: {doc}");
    assert!(
        doc.get("color").is_none(),
        "null must delete the attribute, not store the sentinel: {doc}"
    );

    // one existing + one absent → 207 partition per 5.6.20.5
    let missing = "urn:ngsi-ld:Vehicle:absent-5620";
    let batch = serde_json::json!([
        {"id": ENTITY, "type": "Vehicle", "speed": {"type": "Property", "value": 11}},
        {"id": missing, "type": "Vehicle", "speed": {"type": "Property", "value": 1}}
    ])
    .to_string();
    let res = send(&st, post("/ngsi-ld/v1/entityOperations/merge", batch)).await;
    assert_eq!(res.status(), StatusCode::MULTI_STATUS);
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .expect("body");
    let doc: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(
        doc["success"],
        serde_json::json!([ENTITY]),
        "S array: {doc}"
    );
    let errors = doc["errors"].as_array().expect("E array");
    assert_eq!(errors.len(), 1, "one BatchEntityError: {doc}");
    assert_eq!(errors[0]["entityId"], missing);
    assert_eq!(
        errors[0]["error"]["type"],
        "https://uri.etsi.org/ngsi-ld/errors/ResourceNotFound"
    );

    // 5.5.4 validation: null item and empty array are whole-request 400s
    let res = send(
        &st,
        post(
            "/ngsi-ld/v1/entityOperations/merge",
            format!(r#"[{{"id":"{ENTITY}","type":"Vehicle"}}, null]"#),
        ),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST, "null item");
    let res = send(&st, post("/ngsi-ld/v1/entityOperations/merge", "[]".into())).await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST, "empty array");
}

/// 5.6.9.4 support ladder: no updateBatch → per-entity Update Attributes
/// (overwrite permitted) or per-entity Append Attributes with overwrite
/// disabled (options=noOverwrite).
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_6_9_batch_update_forwarding_fallbacks() {
    let batch_update = |q: &str| {
        let body = serde_json::json!([
            {"id": ENTITY, "type": "Vehicle", "speed": {"type": "Property", "value": 5}}
        ])
        .to_string();
        Request::builder()
            .method("POST")
            .uri(format!("/ngsi-ld/v1/entityOperations/update{q}"))
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request")
    };
    let register_with = |st: AppState, ops: serde_json::Value, port: u16| async move {
        let doc = serde_json::json!({
            "id": "urn:ngsi-ld:ContextSourceRegistration:bu-fb",
            "type": "ContextSourceRegistration",
            "mode": "redirect",
            "operations": ops,
            "information": [{"entities": [{"type": "Vehicle", "id": ENTITY}]}],
            "endpoint": format!("http://127.0.0.1:{port}"),
        });
        let body = doc.to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/csourceRegistrations")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request");
        assert_eq!(send(&st, req).await.status(), StatusCode::CREATED);
        st
    };

    // updateEntity-only source: per-entity Update Attributes forward
    let m = mock_replying("HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n");
    let st = register_with(state(), serde_json::json!(["updateEntity"]), m.port).await;
    let _ = send(&st, batch_update("")).await;
    assert_eq!(m.hits.load(Ordering::SeqCst), 1);
    assert!(
        m.last_head
            .lock()
            .expect("lock")
            .starts_with(&format!("PATCH /ngsi-ld/v1/entities/{ENTITY}/attrs")),
        "overwrite-permitted uses Update Attributes: {}",
        m.last_head.lock().expect("lock")
    );

    // appendAttrs-only source with noOverwrite: per-entity append forward
    let m = mock_replying("HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n");
    let st = register_with(state(), serde_json::json!(["appendAttrs"]), m.port).await;
    let _ = send(&st, batch_update("?options=noOverwrite")).await;
    assert_eq!(m.hits.load(Ordering::SeqCst), 1);
    let head = m.last_head.lock().expect("lock").clone();
    assert!(
        head.starts_with(&format!("POST /ngsi-ld/v1/entities/{ENTITY}/attrs")),
        "noOverwrite uses Append Attributes: {head}"
    );
    assert!(
        head.contains("options=noOverwrite"),
        "append forwarded with overwrite disabled: {head}"
    );
}

/// 5.6.10.4 support ladder: no deleteBatch → per-entity Delete Entity
/// forwards; a proxy source supporting neither is Conflict and never
/// contacted. A null item in the id array fails the whole request (400).
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_6_10_batch_delete_forwarding_fallbacks() {
    let batch_delete = |body: String| {
        Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/entityOperations/delete")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request")
    };
    let register_with = |st: AppState, ops: serde_json::Value, port: u16| async move {
        let doc = serde_json::json!({
            "id": "urn:ngsi-ld:ContextSourceRegistration:bd-fb",
            "type": "ContextSourceRegistration",
            "mode": "redirect",
            "operations": ops,
            "information": [{"entities": [{"type": "Vehicle", "id": ENTITY}]}],
            "endpoint": format!("http://127.0.0.1:{port}"),
        });
        let body = doc.to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/csourceRegistrations")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request");
        assert_eq!(send(&st, req).await.status(), StatusCode::CREATED);
        st
    };

    // deleteEntity-only source: per-entity DELETE forward
    let m = mock_replying("HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n");
    let st = register_with(state(), serde_json::json!(["deleteEntity"]), m.port).await;
    let _ = send(&st, batch_delete(serde_json::json!([ENTITY]).to_string())).await;
    assert_eq!(
        m.hits.load(Ordering::SeqCst),
        1,
        "one Delete Entity forward"
    );
    assert!(
        m.last_head
            .lock()
            .expect("lock")
            .starts_with(&format!("DELETE /ngsi-ld/v1/entities/{ENTITY}")),
        "fallback uses the single-entity resource: {}",
        m.last_head.lock().expect("lock")
    );

    // retrieve-only proxy source: Conflict, never contacted
    let (port, hits) = mock_source();
    let st = register_with(state(), serde_json::json!(["retrieveOps"]), port).await;
    let res = send(&st, batch_delete(serde_json::json!([ENTITY]).to_string())).await;
    let body = String::from_utf8_lossy(
        &axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .expect("body"),
    )
    .into_owned();
    assert!(
        body.contains("does not accept") || body.contains("Conflict"),
        "unsupported proxy source reports Conflict: {body}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0, "never contacted");

    // null item → whole-request 400
    let st = state();
    let res = send(&st, batch_delete(format!(r#"["{ENTITY}", null]"#))).await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

/// 5.6.10.5 Output data: none (empty 204) if all Entities were deleted;
/// otherwise 207 with the S array (deleted ids) and the E array
/// (BatchEntityError per 5.2.17) as {"success": [...], "errors": [...]}.
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_6_10_5_output_shape() {
    let st = state();
    let create = |body: String| {
        Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/entities")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request")
    };
    let batch_delete = |body: String| {
        Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/entityOperations/delete")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request")
    };
    let entity = serde_json::json!({"id": ENTITY, "type": "Vehicle"}).to_string();

    // all deleted → 204 and NO body at all
    assert_eq!(
        send(&st, create(entity.clone())).await.status(),
        StatusCode::CREATED
    );
    let res = send(&st, batch_delete(serde_json::json!([ENTITY]).to_string())).await;
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .expect("body");
    assert!(bytes.is_empty(), "204 carries no body: {bytes:?}");

    // partial → 207 with both arrays, correctly partitioned
    assert_eq!(
        send(&st, create(entity)).await.status(),
        StatusCode::CREATED
    );
    let missing = "urn:ngsi-ld:Vehicle:absent-56105";
    let res = send(
        &st,
        batch_delete(serde_json::json!([ENTITY, missing]).to_string()),
    )
    .await;
    assert_eq!(res.status(), StatusCode::MULTI_STATUS);
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .expect("body");
    let doc: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(
        doc["success"],
        serde_json::json!([ENTITY]),
        "S array holds exactly the deleted ids: {doc}"
    );
    let errors = doc["errors"].as_array().expect("E array");
    assert_eq!(errors.len(), 1, "one BatchEntityError: {doc}");
    assert_eq!(errors[0]["entityId"], missing);
    assert_eq!(
        errors[0]["error"]["type"],
        "https://uri.etsi.org/ngsi-ld/errors/ResourceNotFound"
    );
    // the failed id must NOT leak into S
    assert!(
        !doc["success"]
            .as_array()
            .expect("S array")
            .iter()
            .any(|v| v == missing),
        "missing id must not appear as a success: {doc}"
    );
}

/// 5.6.11.4: exclusive/redirect registrations forward the temporal upsert
/// when "Create or Update Temporal" is supported; unsupported proxy modes
/// are Conflict and never contacted.
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_6_11_temporal_upsert_forwarding() {
    let upsert = || {
        let body = serde_json::json!({
            "id": ENTITY, "type": "Vehicle",
            "speed": [{"type": "Property", "value": 1,
                       "observedAt": "2026-01-01T00:00:00Z"}]
        })
        .to_string();
        Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/temporal/entities")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request")
    };
    let register_with = |st: AppState, ops: serde_json::Value, port: u16| async move {
        let doc = serde_json::json!({
            "id": "urn:ngsi-ld:ContextSourceRegistration:tu-fb",
            "type": "ContextSourceRegistration",
            "mode": "redirect",
            "operations": ops,
            "information": [{"entities": [{"type": "Vehicle", "id": ENTITY}]}],
            "endpoint": format!("http://127.0.0.1:{port}"),
        });
        let body = doc.to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/csourceRegistrations")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request");
        assert_eq!(send(&st, req).await.status(), StatusCode::CREATED);
        st
    };

    // upsertTemporal-supporting source: the upsert is forwarded
    let m = mock_replying("HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n");
    let st = register_with(state(), serde_json::json!(["upsertTemporal"]), m.port).await;
    let _ = send(&st, upsert()).await;
    assert_eq!(
        m.hits.load(Ordering::SeqCst),
        1,
        "temporal upsert forwarded"
    );
    assert!(
        m.last_head
            .lock()
            .expect("lock")
            .starts_with("POST /ngsi-ld/v1/temporal/entities"),
        "forwarded to the temporal resource: {}",
        m.last_head.lock().expect("lock")
    );

    // retrieve-only proxy source: Conflict, never contacted
    let (port, hits) = mock_source();
    let st = register_with(state(), serde_json::json!(["retrieveOps"]), port).await;
    let res = send(&st, upsert()).await;
    let body = String::from_utf8_lossy(
        &axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .expect("body"),
    )
    .into_owned();
    assert!(
        body.contains("does not accept") || body.contains("Conflict"),
        "unsupported proxy source reports Conflict: {body}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0, "never contacted");
}

/// 5.6.12.4: "Add Attributes to Temporal Evolution" forwards to matching
/// registrations supporting appendAttrsTemporal; unsupported proxy modes
/// are Conflict and never contacted.
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_6_12_temporal_add_attrs_forwarding() {
    let add = || {
        let body = serde_json::json!({
            "speed": [{"type": "Property", "value": 2,
                       "observedAt": "2026-01-03T00:00:00Z"}]
        })
        .to_string();
        Request::builder()
            .method("POST")
            .uri(format!("/ngsi-ld/v1/temporal/entities/{ENTITY}/attrs"))
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request")
    };
    let register_with = |st: AppState, ops: serde_json::Value, port: u16| async move {
        let doc = serde_json::json!({
            "id": "urn:ngsi-ld:ContextSourceRegistration:ta-fb",
            "type": "ContextSourceRegistration",
            "mode": "redirect",
            "operations": ops,
            "information": [{"entities": [{"type": "Vehicle", "id": ENTITY}]}],
            "endpoint": format!("http://127.0.0.1:{port}"),
        });
        let body = doc.to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/csourceRegistrations")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request");
        assert_eq!(send(&st, req).await.status(), StatusCode::CREATED);
        st
    };

    let m = mock_replying("HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n");
    let st = register_with(state(), serde_json::json!(["appendAttrsTemporal"]), m.port).await;
    let _ = send(&st, add()).await;
    assert_eq!(
        m.hits.load(Ordering::SeqCst),
        1,
        "temporal add-attrs forwarded"
    );
    assert!(
        m.last_head.lock().expect("lock").starts_with(&format!(
            "POST /ngsi-ld/v1/temporal/entities/{ENTITY}/attrs"
        )),
        "forwarded to the temporal attrs resource: {}",
        m.last_head.lock().expect("lock")
    );

    let (port, hits) = mock_source();
    let st = register_with(state(), serde_json::json!(["retrieveOps"]), port).await;
    let res = send(&st, add()).await;
    let body = String::from_utf8_lossy(
        &axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .expect("body"),
    )
    .into_owned();
    assert!(
        body.contains("does not accept") || body.contains("Conflict"),
        "unsupported proxy source reports Conflict: {body}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0, "never contacted");
}

/// 5.6.13.4/5.6.14.4/5.6.15.4: temporal attribute delete / instance modify /
/// instance delete forward to registrations supporting the respective op;
/// unsupported proxy modes are Conflict and never contacted.
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_6_13_to_15_temporal_attr_ops_forwarding() {
    let register_with = |st: AppState, ops: serde_json::Value, port: u16| async move {
        let doc = serde_json::json!({
            "id": "urn:ngsi-ld:ContextSourceRegistration:td-fb",
            "type": "ContextSourceRegistration",
            "mode": "redirect",
            "operations": ops,
            "information": [{"entities": [{"type": "Vehicle", "id": ENTITY}]}],
            "endpoint": format!("http://127.0.0.1:{port}"),
        });
        let body = doc.to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/csourceRegistrations")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request");
        assert_eq!(send(&st, req).await.status(), StatusCode::CREATED);
        st
    };
    let del_attr = || {
        Request::builder()
            .method("DELETE")
            .uri(format!(
                "/ngsi-ld/v1/temporal/entities/{ENTITY}/attrs/speed"
            ))
            .body(Body::empty())
            .expect("request")
    };

    // supporting source: forwarded
    let m = mock_replying("HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n");
    let st = register_with(state(), serde_json::json!(["deleteAttrsTemporal"]), m.port).await;
    let _ = send(&st, del_attr()).await;
    assert_eq!(
        m.hits.load(Ordering::SeqCst),
        1,
        "temporal attr delete forwarded"
    );
    assert!(
        m.last_head.lock().expect("lock").starts_with(&format!(
            "DELETE /ngsi-ld/v1/temporal/entities/{ENTITY}/attrs/"
        )),
        "forwarded to the temporal attr resource: {}",
        m.last_head.lock().expect("lock")
    );

    // 5.6.14: instance modify forwards as PATCH on the instance resource
    let m = mock_replying("HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n");
    let st = register_with(
        state(),
        serde_json::json!(["updateAttrInstanceTemporal"]),
        m.port,
    )
    .await;
    let body = serde_json::json!({"value": 5}).to_string();
    let req = Request::builder()
        .method("PATCH")
        .uri(format!(
            "/ngsi-ld/v1/temporal/entities/{ENTITY}/attrs/speed/urn:ngsi-ld:Instance:1"
        ))
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .expect("request");
    let _ = send(&st, req).await;
    assert_eq!(
        m.hits.load(Ordering::SeqCst),
        1,
        "instance modify forwarded"
    );
    assert!(
        m.last_head.lock().expect("lock").starts_with(&format!(
            "PATCH /ngsi-ld/v1/temporal/entities/{ENTITY}/attrs/speed/urn:ngsi-ld:Instance:1"
        )),
        "5.6.14 instance path: {}",
        m.last_head.lock().expect("lock")
    );

    // 5.6.15: instance delete forwards as DELETE on the instance resource
    let m = mock_replying("HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n");
    let st = register_with(
        state(),
        serde_json::json!(["deleteAttrInstanceTemporal"]),
        m.port,
    )
    .await;
    let req = Request::builder()
        .method("DELETE")
        .uri(format!(
            "/ngsi-ld/v1/temporal/entities/{ENTITY}/attrs/speed/urn:ngsi-ld:Instance:1"
        ))
        .body(Body::empty())
        .expect("request");
    let _ = send(&st, req).await;
    assert_eq!(
        m.hits.load(Ordering::SeqCst),
        1,
        "instance delete forwarded"
    );
    assert!(
        m.last_head.lock().expect("lock").starts_with(&format!(
            "DELETE /ngsi-ld/v1/temporal/entities/{ENTITY}/attrs/speed/urn:ngsi-ld:Instance:1"
        )),
        "5.6.15 instance path: {}",
        m.last_head.lock().expect("lock")
    );

    // retrieve-only proxy: Conflict, never contacted
    let (port, hits) = mock_source();
    let st = register_with(state(), serde_json::json!(["retrieveOps"]), port).await;
    let res = send(&st, del_attr()).await;
    let body = String::from_utf8_lossy(
        &axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .expect("body"),
    )
    .into_owned();
    assert!(
        body.contains("does not accept") || body.contains("Conflict"),
        "unsupported proxy source reports Conflict: {body}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0, "never contacted");
}

/// 5.6.16.4: Delete Temporal Evolution forwards to registrations supporting
/// deleteTemporal; unsupported proxy modes are Conflict, never contacted.
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_6_16_temporal_delete_forwarding() {
    let register_with = |st: AppState, ops: serde_json::Value, port: u16| async move {
        let doc = serde_json::json!({
            "id": "urn:ngsi-ld:ContextSourceRegistration:tdel-fb",
            "type": "ContextSourceRegistration",
            "mode": "redirect",
            "operations": ops,
            "information": [{"entities": [{"type": "Vehicle", "id": ENTITY}]}],
            "endpoint": format!("http://127.0.0.1:{port}"),
        });
        let body = doc.to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/csourceRegistrations")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request");
        assert_eq!(send(&st, req).await.status(), StatusCode::CREATED);
        st
    };
    let del = || {
        Request::builder()
            .method("DELETE")
            .uri(format!("/ngsi-ld/v1/temporal/entities/{ENTITY}"))
            .body(Body::empty())
            .expect("request")
    };

    let m = mock_replying("HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n");
    let st = register_with(state(), serde_json::json!(["deleteTemporal"]), m.port).await;
    let _ = send(&st, del()).await;
    assert_eq!(
        m.hits.load(Ordering::SeqCst),
        1,
        "temporal delete forwarded"
    );
    assert!(
        m.last_head
            .lock()
            .expect("lock")
            .starts_with(&format!("DELETE /ngsi-ld/v1/temporal/entities/{ENTITY} ")),
        "forwarded to the temporal entity resource: {}",
        m.last_head.lock().expect("lock")
    );

    let (port, hits) = mock_source();
    let st = register_with(state(), serde_json::json!(["retrieveOps"]), port).await;
    let res = send(&st, del()).await;
    let body = String::from_utf8_lossy(
        &axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .expect("body"),
    )
    .into_owned();
    assert!(
        body.contains("does not accept") || body.contains("Conflict"),
        "unsupported proxy source reports Conflict: {body}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0, "never contacted");
}

/// 5.6.17.4: a redirect registration without Merge Entity support is
/// Conflict (never contacted); inclusive-unsupported is skipped while the
/// local merge proceeds.
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_6_17_merge_unsupported_by_registration() {
    let merge = || {
        let body = serde_json::json!({"speed": {"type": "Property", "value": 7}}).to_string();
        Request::builder()
            .method("PATCH")
            .uri(format!("/ngsi-ld/v1/entities/{ENTITY}"))
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request")
    };
    let register_with = |st: AppState, mode: &'static str, port: u16| async move {
        let doc = serde_json::json!({
            "id": format!("urn:ngsi-ld:ContextSourceRegistration:mrg-{mode}"),
            "type": "ContextSourceRegistration",
            "mode": mode,
            "operations": ["retrieveOps"],
            "information": [{"entities": [{"type": "Vehicle", "id": ENTITY}]}],
            "endpoint": format!("http://127.0.0.1:{port}"),
        });
        let body = doc.to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/csourceRegistrations")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request");
        assert_eq!(send(&st, req).await.status(), StatusCode::CREATED);
        st
    };

    let (port, hits) = mock_source();
    let st = register_with(state(), "redirect", port).await;
    let res = send(&st, merge()).await;
    assert_eq!(res.status(), StatusCode::CONFLICT, "complete merge failed");
    assert_eq!(hits.load(Ordering::SeqCst), 0, "never contacted");

    // inclusive without support: local merge proceeds, no forward
    let (port, hits) = mock_source();
    let st = register_with(state(), "inclusive", port).await;
    let body = serde_json::json!({"id": ENTITY, "type": "Vehicle",
        "speed": {"type": "Property", "value": 1}})
    .to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/ngsi-ld/v1/entities?local=true")
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .expect("request");
    assert_eq!(send(&st, req).await.status(), StatusCode::CREATED);
    let res = send(&st, merge()).await;
    assert_eq!(res.status(), StatusCode::NO_CONTENT, "local merge proceeds");
    assert_eq!(hits.load(Ordering::SeqCst), 0, "no forward without support");
}

/// 5.6.18.4: replace honours the ?type selector (mismatch → 404, entity
/// intact) and a redirect registration without Replace Entity support is
/// Conflict, never contacted.
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_6_18_replace_type_selector_and_unsupported() {
    let put = |q: &str| {
        let body = serde_json::json!({"id": ENTITY, "type": "Vehicle",
            "speed": {"type": "Property", "value": 9}})
        .to_string();
        Request::builder()
            .method("PUT")
            .uri(format!("/ngsi-ld/v1/entities/{ENTITY}{q}"))
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request")
    };
    // type selector: create locally, replace with wrong selector → 404
    let st = state();
    let body = serde_json::json!({"id": ENTITY, "type": "Vehicle",
        "speed": {"type": "Property", "value": 1}})
    .to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/ngsi-ld/v1/entities")
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .expect("request");
    assert_eq!(send(&st, req).await.status(), StatusCode::CREATED);
    let res = send(&st, put("?type=Building")).await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND, "wrong selector is 404");
    let res = send(&st, put("?type=Vehicle")).await;
    assert_eq!(
        res.status(),
        StatusCode::NO_CONTENT,
        "matching selector replaces"
    );

    // redirect without replaceEntity: Conflict, never contacted
    let (port, hits) = mock_source();
    let st = state();
    let doc = serde_json::json!({
        "id": "urn:ngsi-ld:ContextSourceRegistration:rep-fb",
        "type": "ContextSourceRegistration",
        "mode": "redirect",
        "operations": ["retrieveOps"],
        "information": [{"entities": [{"type": "Vehicle", "id": ENTITY}]}],
        "endpoint": format!("http://127.0.0.1:{port}"),
    });
    let body = doc.to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/ngsi-ld/v1/csourceRegistrations")
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .expect("request");
    assert_eq!(send(&st, req).await.status(), StatusCode::CREATED);
    let res = send(&st, put("")).await;
    assert_eq!(
        res.status(),
        StatusCode::CONFLICT,
        "complete replace failed"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0, "never contacted");
}

/// 5.6.13-5.6.16 answer through the same 6.3.17/6.3.18 loop check as every
/// other distributed write. A 508 is an error status: it tells the client the
/// operation did not take place, so the local Temporal Evolution has to be
/// exactly what it was before the request arrived.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_loop_leaves_the_temporal_evolution_untouched() {
    let st = state();
    let body = serde_json::json!({
        "id": ENTITY,
        "type": "Vehicle",
        "speed": [{"type": "Property", "value": 120, "observedAt": "2020-09-01T12:03:00Z"}],
    })
    .to_string();
    let res = send(
        &st,
        Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/temporal/entities")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request"),
    )
    .await;
    assert_eq!(res.status(), StatusCode::CREATED, "temporal create");

    let read = || async {
        let res = send(
            &st,
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/ngsi-ld/v1/temporal/entities/{ENTITY}\
                     ?timerel=before&timeAt=2030-01-01T00:00:00Z&options=sysAttrs"
                ))
                .body(Body::empty())
                .expect("request"),
        )
        .await;
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .expect("body");
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or(serde_json::Value::Null)
    };

    let before = read().await;
    let iid = before["speed"][0]["instanceId"]
        .as_str()
        .expect("instanceId")
        .to_owned();

    // A Via chain past the broker's hop ceiling is refused before any source
    // is consulted, so the 508 is reachable without a registration at all.
    let via = vec!["1.1 a"; 33].join(", ");
    let fragment = serde_json::json!({"type": "Property", "value": 129}).to_string();
    let cases = [
        (
            "DELETE",
            format!("/ngsi-ld/v1/temporal/entities/{ENTITY}"),
            None,
        ),
        (
            "DELETE",
            format!("/ngsi-ld/v1/temporal/entities/{ENTITY}/attrs/speed?deleteAll=true"),
            None,
        ),
        (
            "PATCH",
            format!("/ngsi-ld/v1/temporal/entities/{ENTITY}/attrs/speed/{iid}"),
            Some(fragment),
        ),
        (
            "DELETE",
            format!("/ngsi-ld/v1/temporal/entities/{ENTITY}/attrs/speed/{iid}"),
            None,
        ),
    ];
    for (method, uri, payload) in cases {
        let mut req = Request::builder()
            .method(method)
            .uri(&uri)
            .header("Via", &via);
        let sent = match payload {
            Some(p) => {
                req = req
                    .header("Content-Type", "application/json")
                    .header("Content-Length", p.len());
                Body::from(p)
            }
            None => Body::empty(),
        };
        let res = send(&st, req.body(sent).expect("request")).await;
        assert_eq!(res.status(), StatusCode::LOOP_DETECTED, "{method} {uri}");
        assert_eq!(
            read().await,
            before,
            "a refused {method} {uri} still changed the stored Temporal Evolution"
        );
    }
}

/// 5.6.21 Purge Entities answers through the same loop check, and it deletes
/// a whole page rather than one Entity — the same 508 that means "nothing
/// happened" was returned over an emptied store.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_loop_leaves_the_purged_entities_in_place() {
    let st = state();
    for i in 0..3 {
        let body =
            serde_json::json!({"id": format!("urn:ngsi-ld:Vehicle:p{i}"), "type": "Vehicle"})
                .to_string();
        let res = send(
            &st,
            Request::builder()
                .method("POST")
                .uri("/ngsi-ld/v1/entities")
                .header("Content-Type", "application/json")
                .header("Content-Length", body.len())
                .body(Body::from(body))
                .expect("request"),
        )
        .await;
        assert_eq!(res.status(), StatusCode::CREATED, "seed {i}");
    }
    let count = || async {
        let res = send(
            &st,
            Request::builder()
                .method("GET")
                .uri("/ngsi-ld/v1/entities?type=Vehicle")
                .body(Body::empty())
                .expect("request"),
        )
        .await;
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .expect("body");
        serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()
            .and_then(|v| v.as_array().map(Vec::len))
            .unwrap_or_default()
    };
    assert_eq!(count().await, 3, "seeded");

    let res = send(
        &st,
        Request::builder()
            .method("DELETE")
            .uri("/ngsi-ld/v1/entities?type=Vehicle")
            .header("Via", vec!["1.1 a"; 33].join(", "))
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::LOOP_DETECTED,
        "purge under a loop"
    );
    assert_eq!(
        count().await,
        3,
        "a refused purge still emptied the Entity store"
    );
}
