// SPDX-License-Identifier: EUPL-1.2
//! Tiles Part 1 (T1 to T6, T-5121): a collection's vector tiles are its `items` through the
//! view, so the view's rules, its `q` and the Policy narrow them as they narrow a page.

use crate::common::{fixture, fixture_with, get, href, schemas_on};
use antares_ops::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tower::ServiceExt;

const CITY: &str = "/x/views/city/ogc/features";
const TILE: &str = "/x/views/city/ogc/features/collections/Car/tiles/WebMercatorQuad";

/// A request's status, headers and raw body.
async fn raw(
    app: &Router,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut req = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let Ok(resp) = app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap_or_default())
        .await;
    let (status, headers) = (resp.status(), resp.headers().clone());
    let body = resp
        .into_body()
        .collect()
        .await
        .map(|b| b.to_bytes().to_vec())
        .unwrap_or_default();
    (status, headers, body)
}

/// The fields of one protobuf message: (field, payload of a length-delimited one, varint value).
fn fields(mut b: &[u8]) -> Vec<(u64, Vec<u8>, u64)> {
    fn varint(b: &mut &[u8]) -> u64 {
        let mut v = 0u64;
        let mut shift = 0u32;
        while let Some((byte, rest)) = b.split_first() {
            *b = rest;
            v |= u64::from(byte & 0x7f).wrapping_shl(shift);
            shift = shift.wrapping_add(7);
            if byte & 0x80 == 0 {
                break;
            }
        }
        v
    }
    let mut out = Vec::new();
    while !b.is_empty() {
        let k = varint(&mut b);
        match k & 7 {
            0 => out.push((k.wrapping_shr(3), Vec::new(), varint(&mut b))),
            2 => {
                let n = usize::try_from(varint(&mut b)).unwrap_or(0);
                let (payload, rest) = b.split_at(n.min(b.len()));
                out.push((k.wrapping_shr(3), payload.to_vec(), 0));
                b = rest;
            }
            _ => {
                let (payload, rest) = b.split_at(8.min(b.len()));
                out.push((k.wrapping_shr(3), payload.to_vec(), 0));
                b = rest;
            }
        }
    }
    out
}

/// The layer of a tile: its name, version, extent, feature count and keys.
fn layer(tile: &[u8]) -> (String, u64, u64, usize, Vec<String>) {
    let top = fields(tile);
    assert_eq!(top.len(), 1, "one layer");
    let layer = fields(&top.first().map(|f| f.1.clone()).unwrap_or_default());
    let text = |f: &(u64, Vec<u8>, u64)| String::from_utf8_lossy(&f.1).into_owned();
    let of = |n: u64| layer.iter().filter(move |f| f.0 == n);
    (
        of(1).map(text).collect(),
        of(15).map(|f| f.2).sum(),
        of(5).map(|f| f.2).sum(),
        of(2).count(),
        of(3).map(text).collect(),
    )
}

fn holds(body: &[u8], needle: &str) -> bool {
    body.windows(needle.len()).any(|w| w == needle.as_bytes())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tile_keeps_the_shown_attributes_and_never_a_hidden_one() -> Result<(), String> {
    let f = fixture().await?;
    let (status, headers, body) = raw(&f.app, "GET", &format!("{TILE}/0/0/0"), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get("content-type").and_then(|v| v.to_str().ok()),
        Some("application/vnd.mapbox-vector-tile")
    );
    for shown in [
        "Car",
        "speed",
        "urn:ngsi-ld:Car:1",
        "urn:ngsi-ld:Car:2",
        "urn:ngsi-ld:Car:4",
    ] {
        assert!(holds(&body, shown), "{shown}");
    }
    for hidden in ["secret", "s1", "s2", "urn:ngsi-ld:Car:3"] {
        assert!(!holds(&body, hidden), "{hidden}");
    }
    assert_eq!(headers.get("antares-tile-truncated"), None);
    let (name, version, extent, features, mut keys) = layer(&body);
    keys.sort();
    assert_eq!(
        (name.as_str(), version, extent, features),
        ("Car", 2, 4096, 3)
    );
    assert_eq!(
        keys,
        ["at", "colour", "home", "id", "seen", "speed", "tags"]
    );
    // Bratislava's tile at matrix 10 holds Car 1 alone; the empty tile beside the pole is 204.
    let (status, _, body) = raw(&f.app, "GET", &format!("{TILE}/10/355/560"), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(holds(&body, "urn:ngsi-ld:Car:1"));
    assert!(!holds(&body, "urn:ngsi-ld:Car:2"));
    let (status, _, body) = raw(&f.app, "GET", &format!("{TILE}/10/0/0"), &[]).await;
    assert_eq!((status, body.len()), (StatusCode::NO_CONTENT, 0));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_view_narrows_a_tile_as_it_narrows_a_page() -> Result<(), String> {
    let f = fixture().await?;
    let slow = "/x/views/slow/ogc/features/collections/Car/tiles/WebMercatorQuad/0/0/0";
    let (status, _, body) = raw(&f.app, "GET", slow, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(holds(&body, "urn:ngsi-ld:Car:1"));
    assert!(!holds(&body, "urn:ngsi-ld:Car:2"));
    assert!(!holds(&body, "urn:ngsi-ld:Car:4"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tile_is_revalidated_by_its_etag() -> Result<(), String> {
    let f = fixture().await?;
    let (_, headers, _) = raw(&f.app, "GET", &format!("{TILE}/0/0/0"), &[]).await;
    let etag = headers
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert!(etag.starts_with("W/\""), "{etag}");
    assert_eq!(
        headers.get("cache-control").and_then(|v| v.to_str().ok()),
        Some("private, max-age=60")
    );
    let (status, _, body) = raw(
        &f.app,
        "GET",
        &format!("{TILE}/0/0/0"),
        &[("If-None-Match", &etag)],
    )
    .await;
    assert_eq!((status, body.len()), (StatusCode::NOT_MODIFIED, 0));
    let (status, headers, body) = raw(&f.app, "HEAD", &format!("{TILE}/0/0/0"), &[]).await;
    assert_eq!((status, body.len()), (StatusCode::OK, 0));
    assert!(headers.get("content-length").is_some());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tile_outside_the_set_or_the_caps_is_refused() -> Result<(), String> {
    let f = fixture().await?;
    let stop = "/x/views/city/ogc/features/collections/Stop/tiles/WebMercatorQuad/0/0/0";
    for (uri, accept, want) in [
        (format!("{TILE}/23/0/0"), "*/*", StatusCode::NOT_FOUND),
        (format!("{TILE}/2/4/0"), "*/*", StatusCode::NOT_FOUND),
        (format!("{TILE}/2/0/x"), "*/*", StatusCode::NOT_FOUND),
        (stop.to_owned(), "*/*", StatusCode::NOT_FOUND),
        (
            "/x/views/city/ogc/features/collections/Depot/tiles/WebMercatorQuad/0/0/0".to_owned(),
            "*/*",
            StatusCode::NOT_FOUND,
        ),
        (
            format!("{TILE}/0/0/0?limit=5"),
            "*/*",
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("{TILE}/0/0/0"),
            "image/png",
            StatusCode::NOT_ACCEPTABLE,
        ),
        (
            format!("{TILE}/0/0/0"),
            "application/vnd.mapbox-vector-tile",
            StatusCode::OK,
        ),
        (
            format!("{TILE}/0/0/0"),
            "application/*;q=0.5",
            StatusCode::OK,
        ),
        (format!("{TILE}/0/0/0"), "image/png, */*", StatusCode::OK),
        (TILE.to_owned(), "image/png", StatusCode::NOT_ACCEPTABLE),
        (TILE.to_owned(), "application/json", StatusCode::OK),
    ] {
        let (status, _, _) = raw(&f.app, "GET", &uri, &[("Accept", accept)]).await;
        assert_eq!(status, want, "{uri}");
    }
    Ok(())
}

/// An engine that, once locked, refuses every query (clause 5.7) as a Policy without a read
/// grant would.
#[derive(Default)]
struct NoReads(AtomicBool);

impl PolicyEngine for NoReads {
    fn name(&self) -> &'static str {
        "no-reads"
    }

    fn decide<'a>(&'a self, _: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let locked = self.0.load(Ordering::SeqCst);
        Box::pin(std::future::ready(
            if locked && op.clause.starts_with("5.7.") {
                Decision::Deny("no read".into())
            } else {
                Decision::Allow
            },
        ))
    }

    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_policy_without_reads_refuses_every_tile() -> Result<(), String> {
    schemas_on();
    let engine = Arc::new(NoReads::default());
    let f = fixture_with(TestBroker::new().await?.with_policy(engine.clone())).await?;
    engine.0.store(true, Ordering::SeqCst);
    let (status, _, body) = raw(&f.app, "GET", &format!("{TILE}/0/0/0"), &[]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(!holds(&body, "urn:ngsi-ld:Car"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_tileset_names_its_matrix_set_and_its_tile_template() -> Result<(), String> {
    let f = fixture().await?;
    let car = get(&f.app, &format!("{CITY}/collections/Car")).await.body;
    let tilesets = href(
        &car,
        "http://www.opengis.net/def/rel/ogc/1.0/tilesets-vector",
    );
    assert_eq!(
        tilesets.as_deref(),
        Some("http://localhost:9090/x/views/city/ogc/features/collections/Car/tiles")
    );
    let stop = get(&f.app, &format!("{CITY}/collections/Stop")).await.body;
    assert_eq!(
        href(
            &stop,
            "http://www.opengis.net/def/rel/ogc/1.0/tilesets-vector"
        ),
        None
    );
    let list = get(&f.app, &format!("{CITY}/collections/Car/tiles")).await;
    assert_eq!(list.status, StatusCode::OK);
    assert_eq!(
        list.body.pointer("/tilesets/0/tileMatrixSetURI"),
        Some(&json!(
            "http://www.opengis.net/def/tilematrixset/OGC/1.0/WebMercatorQuad"
        ))
    );
    let set = get(&f.app, TILE).await.body;
    assert_eq!(set.get("dataType"), Some(&json!("vector")));
    assert_eq!(
        href(&set, "item").as_deref(),
        Some("http://localhost:9090/x/views/city/ogc/features/collections/Car/tiles/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}")
    );
    let scheme = get(&f.app, &format!("{CITY}/tileMatrixSets/WebMercatorQuad"))
        .await
        .body;
    assert_eq!(
        scheme.pointer("/tileMatrices/1/matrixWidth"),
        Some(&json!(2))
    );
    let schemes = get(&f.app, &format!("{CITY}/tileMatrixSets")).await;
    assert_eq!(
        schemes.body.pointer("/tileMatrixSets/0/id"),
        Some(&json!("WebMercatorQuad"))
    );
    assert_eq!(
        get(&f.app, &format!("{CITY}/tileMatrixSets?x=1"))
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(&f.app, &format!("{CITY}/collections/Stop/tiles"))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    let classes = get(&f.app, &format!("{CITY}/conformance"))
        .await
        .body
        .to_string();
    assert!(classes.contains("ogcapi-tiles-1/1.0/conf/mvt"));
    let api = get(&f.app, &format!("{CITY}/api")).await.body;
    // Tiles Part 1 /req/oas30/operation-id, Table 11: the ETS looks for these names.
    let ids = api.to_string();
    for id in [
        "Car.collection.vector.getTileSetsList",
        "Car.collection.vector.getTileSet",
        "Car.collection.vector.getTile",
        "getTileMatrixSetsList",
        "getTileMatrixSet",
    ] {
        assert!(ids.contains(&format!("\"{id}\"")), "{id}");
    }
    assert!(api
        .pointer("/paths/~1collections~1Car~1tiles~1WebMercatorQuad~1{tileMatrix}~1{tileRow}~1{tileCol}/get/responses/204")
        .is_some());
    Ok(())
}
