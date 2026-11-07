// SPDX-License-Identifier: EUPL-1.2
//! T-5847: responses honour Accept-Encoding gzip and br. The decoded body is
//! byte-identical to the plain one, a tiny body stays plain, and a request
//! body is never decompressed. The layer is the one the binary lays over
//! the router (`antares_api::compression`); `crates/antares-broker/tests/
//! compression.rs` proves the binary serves it.

use std::io::Read;

use antares_api::AppState;
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn send(st: &AppState, req: Request<Body>) -> (StatusCode, HeaderMap, Bytes) {
    let resp = antares_api::router(st.clone())
        .layer(antares_api::compression())
        .oneshot(req)
        .await
        .expect("response");
    let (parts, body) = resp.into_parts();
    let bytes = body.collect().await.expect("body").to_bytes();
    (parts.status, parts.headers, bytes)
}

fn get(uri: &str, encoding: Option<&str>) -> Request<Body> {
    let mut req = Request::builder().uri(uri);
    if let Some(e) = encoding {
        req = req.header("Accept-Encoding", e);
    }
    req.body(Body::empty()).expect("request")
}

/// Ten entities of one type: the attribute names repeat, as in a real page.
async fn seeded() -> AppState {
    let st = AppState::new("test".into());
    for n in 0..10 {
        let create = format!(
            r#"{{"id":"urn:ngsi-ld:Kpi:{n}","type":"Kpi",
            "kpiValue":{{"type":"Property","value":{n}}},
            "description":{{"type":"Property","value":"share of buses on time"}}}}"#
        );
        let (status, _, body) = send(
            &st,
            Request::builder()
                .method("POST")
                .uri("/ngsi-ld/v1/entities")
                .header("Content-Type", "application/json")
                .header("Content-Length", create.len())
                .body(Body::from(create))
                .expect("request"),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body:?}");
    }
    st
}

fn header<'a>(h: &'a HeaderMap, name: &str) -> Vec<&'a str> {
    h.get_all(name)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect()
}

fn gunzip(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(b)
        .read_to_end(&mut out)
        .expect("gzip");
    out
}

fn unbrotli(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    brotli::Decompressor::new(b, 4096)
        .read_to_end(&mut out)
        .expect("br");
    out
}

/// Asks `uri` plain and with `encoding`; returns the status of both reads, the
/// plain body and the decoded one.
async fn both(st: &AppState, uri: &str, encoding: &str) -> (StatusCode, Bytes, Vec<u8>) {
    let (status, plain_headers, plain) = send(st, get(uri, None)).await;
    assert!(
        plain_headers.get("content-encoding").is_none(),
        "no Accept-Encoding, yet {plain_headers:?}"
    );
    let (status_enc, headers, packed) = send(st, get(uri, Some(encoding))).await;
    assert_eq!(status, status_enc);
    assert_eq!(header(&headers, "content-encoding"), vec![encoding]);
    let vary = header(&headers, "vary").join(",").to_ascii_lowercase();
    assert!(vary.contains("accept-encoding"), "vary: {vary}");
    assert!(
        packed.len() < plain.len(),
        "{} >= {}",
        packed.len(),
        plain.len()
    );
    let decoded = match encoding {
        "gzip" => gunzip(&packed),
        _ => unbrotli(&packed),
    };
    (status, plain, decoded)
}

#[tokio::test(flavor = "multi_thread")]
async fn gzip_answer_decodes_to_the_plain_body() {
    let st = seeded().await;
    let (status, plain, decoded) = both(&st, "/ngsi-ld/v1/entities?type=Kpi", "gzip").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(decoded, plain.to_vec());
}

#[tokio::test(flavor = "multi_thread")]
async fn br_answer_decodes_to_the_plain_body() {
    let st = seeded().await;
    let (status, plain, decoded) = both(&st, "/ngsi-ld/v1/entities?type=Kpi", "br").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(decoded, plain.to_vec());
}

/// A problem document is a few hundred bytes: under the 1 KiB minimum
/// (T-5849) it goes out plain even to a client that asks for gzip.
#[tokio::test(flavor = "multi_thread")]
async fn a_problem_answer_stays_plain() {
    let st = AppState::new("test".into());
    let (status, headers, body) = send(
        &st,
        get("/ngsi-ld/v1/entities/urn:ngsi-ld:Kpi:missing", Some("gzip")),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(headers.get("content-encoding").is_none(), "{headers:?}");
    let problem: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(
        problem["type"],
        "https://uri.etsi.org/ngsi-ld/errors/ResourceNotFound"
    );
}

/// A body of known size under the minimum (here 6.3.4's bare 411) stays
/// plain. A streamed list has no known size, so even an empty one is packed.
#[tokio::test(flavor = "multi_thread")]
async fn a_body_under_the_minimum_stays_plain() {
    let st = AppState::new("test".into());
    let (status, headers, body) = send(
        &st,
        Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/entities")
            .header("Content-Type", "application/json")
            .header("Accept-Encoding", "gzip, br")
            .body(Body::from("{}"))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::LENGTH_REQUIRED);
    assert!(body.len() <= 32, "{body:?}");
    assert!(headers.get("content-encoding").is_none(), "{headers:?}");
}

/// No request decompression: a gzip body is not unpacked past the body
/// bound, so it is refused and nothing is stored.
#[tokio::test(flavor = "multi_thread")]
async fn a_gzip_request_body_is_refused() {
    use std::io::Write;
    let st = AppState::new("test".into());
    let entity = br#"{"id":"urn:ngsi-ld:Kpi:z","type":"Kpi"}"#;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(entity).expect("gzip");
    let packed = enc.finish().expect("gzip");
    let (status, _, body) = send(
        &st,
        Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/entities")
            .header("Content-Type", "application/json")
            .header("Content-Encoding", "gzip")
            .header("Content-Length", packed.len())
            .body(Body::from(packed))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
    let (status, _, _) = send(&st, get("/ngsi-ld/v1/entities/urn:ngsi-ld:Kpi:z", None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// T-5849: brotli quality 2, gzip level 1 and a 1 KiB minimum. The handler
// below answers a fixed body, so each case sets the exact size and type.

fn fixed(body: Vec<u8>, content_type: &'static str) -> axum::Router {
    axum::Router::new()
        .route(
            "/",
            axum::routing::get(move || {
                let body = body.clone();
                async move { ([("content-type", content_type)], body) }
            }),
        )
        .layer(antares_api::compression())
}

async fn fetch(app: axum::Router, encoding: &str) -> (HeaderMap, Vec<u8>) {
    let resp = app
        .oneshot(get("/", Some(encoding)))
        .await
        .expect("response");
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (headers, bytes.to_vec())
}

/// A page of entities as a broker answers it: repeated attribute names,
/// varying values, about 20 KB.
fn page() -> Vec<u8> {
    let rows: Vec<String> = (0..80)
        .map(|n| {
            format!(
                r#"{{"id":"urn:ngsi-ld:BikeHireDockingStation:{n:03}","type":"BikeHireDockingStation","availableBikeNumber":{{"type":"Property","value":{}}},"location":{{"type":"GeoProperty","value":{{"type":"Point","coordinates":[24.{:04},60.{:04}]}}}}}}"#,
                n * 7 % 23,
                n * 131 % 9973,
                n * 89 % 9967
            )
        })
        .collect();
    format!("[{}]", rows.join(",")).into_bytes()
}

fn json_of(len: usize) -> Vec<u8> {
    let mut body = page();
    body.truncate(len);
    body
}

#[tokio::test]
async fn a_900_byte_body_stays_plain() {
    let body = json_of(900);
    let (headers, got) = fetch(fixed(body.clone(), "application/json"), "gzip, br").await;
    assert!(headers.get("content-encoding").is_none(), "{headers:?}");
    assert_eq!(got, body);
}

#[tokio::test]
async fn a_2_kib_body_is_packed() {
    let body = json_of(2048);
    let (headers, got) = fetch(fixed(body.clone(), "application/json"), "gzip").await;
    assert_eq!(header(&headers, "content-encoding"), vec!["gzip"]);
    assert_eq!(gunzip(&got), body);
}

#[tokio::test]
async fn br_wins_when_both_are_offered() {
    for offer in ["gzip, br", "br, gzip", "gzip;q=1, br;q=0.9"] {
        let (headers, got) = fetch(fixed(page(), "application/json"), offer).await;
        assert_eq!(header(&headers, "content-encoding"), vec!["br"], "{offer}");
        let vary = header(&headers, "vary").join(",").to_ascii_lowercase();
        assert_eq!(vary.matches("accept-encoding").count(), 1, "{vary}");
        assert_eq!(unbrotli(&got), page());
    }
}

#[tokio::test]
async fn event_streams_and_images_stay_plain() {
    for content_type in ["text/event-stream", "image/png"] {
        let body = json_of(4096);
        let (headers, got) = fetch(fixed(body.clone(), content_type), "gzip, br").await;
        assert!(
            headers.get("content-encoding").is_none(),
            "{content_type}: {headers:?}"
        );
        assert_eq!(got, body);
    }
}

#[tokio::test]
async fn gzip_runs_at_level_1() {
    use std::io::Write;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(1));
    enc.write_all(&page()).expect("gzip");
    let want = enc.finish().expect("gzip");
    let (headers, got) = fetch(fixed(page(), "application/json"), "gzip").await;
    assert_eq!(header(&headers, "content-encoding"), vec!["gzip"]);
    assert_eq!(got, want);
}

#[tokio::test]
async fn br_runs_at_quality_2() {
    use std::io::Write;
    let mut enc = brotli::CompressorWriter::new(Vec::new(), 4096, 2, 22);
    enc.write_all(&page()).expect("br");
    let want = enc.into_inner();
    let (headers, got) = fetch(fixed(page(), "application/json"), "br").await;
    assert_eq!(header(&headers, "content-encoding"), vec!["br"]);
    assert_eq!(got, want);
}
